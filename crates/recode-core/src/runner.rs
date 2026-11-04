use crate::engine::Engine;
use crate::prompt;
use futures_util::StreamExt;
use recode_schema::event::{DeltaKind, Event, LiveEvent, SessionStatus};
use recode_schema::id::{MessageId, PartId, SessionId};
use recode_schema::llm::{FinishReason, LlmEvent, LlmRequest, ToolChoice, Usage};
use recode_schema::message::{MessageError, MessageInfo};
use recode_schema::model::ModelRef;
use recode_schema::part::{
    ErrorPart, Part, PartTime, PatchPart, StepFinishPart, StepStartPart, ToolPart, ToolState,
    ToolStatus,
};
use recode_schema::permission::Decision;
use recode_schema::tool::ToolOutcome;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const MAX_STEPS: usize = 64;
const DELTA_FLUSH_BYTES: usize = 2048;
const DELTA_FLUSH_MS: u128 = 60;

pub async fn run(
    engine: Arc<Engine>,
    session_id: SessionId,
    agent_id: String,
    model: ModelRef,
    cancel: CancellationToken,
) {
    engine.set_status(&session_id, SessionStatus::Busy);
    let result = run_inner(&engine, &session_id, &agent_id, &model, &cancel).await;
    match result {
        Ok(()) => {
            record_patch(&engine, &session_id).await;
            let _ = engine
                .store
                .append(Event::SessionIdle {
                    session_id: session_id.clone(),
                })
                .map(|envelope| {
                    let _ = engine.bus.send(LiveEvent::Durable(envelope));
                });
        }
        Err(error) => {
            tracing::warn!(session = %session_id, %error, "session run failed");
            let message_error = MessageError::new("SessionError", &error, false);
            let _ = engine.store.append(Event::SessionError {
                session_id: session_id.clone(),
                error: message_error,
            });
            let last_assistant = engine.messages(&session_id).ok().and_then(|messages| {
                messages
                    .into_iter()
                    .rev()
                    .find(|message| message.info.role == recode_schema::Role::Assistant)
            });
            if let Some(message) = last_assistant {
                let part = Part::Error(ErrorPart {
                    id: PartId::new(),
                    message: error,
                    name: Some("SessionError".into()),
                });
                let _ = engine.store.append(Event::PartAdded {
                    session_id: session_id.clone(),
                    message_id: message.info.id,
                    part,
                });
            }
        }
    }
    engine.set_status(&session_id, SessionStatus::Idle);
}

/// Record the files changed during this run on the last assistant message.
/// The hash points at the pre-prompt snapshot, so a client can diff or restore
/// from the same boundary.
async fn record_patch(engine: &Arc<Engine>, session_id: &SessionId) {
    let Some(snapshot) = engine.latest_snapshot(session_id) else {
        return;
    };
    let files = match engine.snapshots.changed_files(&snapshot).await {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(%error, "cannot compute changed files");
            return;
        }
    };
    if files.is_empty() {
        return;
    }
    let Some(message) = engine.messages(session_id).ok().and_then(|messages| {
        messages
            .into_iter()
            .rev()
            .find(|message| message.info.role == recode_schema::Role::Assistant)
    }) else {
        return;
    };
    let part = Part::Patch(PatchPart {
        id: PartId::new(),
        hash: snapshot,
        files,
    });
    match engine.store.append(Event::PartAdded {
        session_id: session_id.clone(),
        message_id: message.info.id,
        part,
    }) {
        Ok(envelope) => {
            let _ = engine.bus.send(LiveEvent::Durable(envelope));
        }
        Err(error) => tracing::warn!(%error, "cannot persist patch part"),
    }
}

async fn run_inner(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    agent_id: &str,
    model: &ModelRef,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let session = engine
        .session(session_id)?
        .ok_or_else(|| "session not found".to_owned())?;
    let agent = engine.agent(agent_id);
    let instructions = engine.instructions_for(&session);
    let system = prompt::system_prompt(
        agent,
        &instructions,
        &engine.project,
        model,
        &session.directory,
    );
    let provider = engine
        .providers
        .get(&model.provider_id)
        .map_err(|error| error.to_string())?;
    let model_config = engine.config.model_config(model);
    let allow_outside =
        engine.config.ruleset().evaluate("external_directory", "*") == Some(Decision::Allow);
    let tool_specs = engine.tools.specs(if agent.tools.is_empty() {
        None
    } else {
        Some(&agent.tools)
    });
    let max_tokens = model_config.limit.output.clamp(256, 32_000) as u32;
    let temperature = agent.temperature.or(model_config.temperature);

    for step in 0..MAX_STEPS {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let messages = engine.messages(session_id)?;
        let stored: Vec<recode_storage::StoredMessage> = messages
            .iter()
            .map(|message| recode_storage::StoredMessage {
                info: message.info.clone(),
                parts: message.parts.clone(),
            })
            .collect();
        let compaction = engine
            .store
            .compaction(session_id)
            .map_err(|error| error.to_string())?;
        let mut llm_messages = prompt::to_llm_messages(&stored, compaction.as_ref());
        let context_limit = model_config.limit.context.max(8_000);
        let estimated = crate::compaction::estimate_tokens(&llm_messages)
            + (system.chars().count() as u64 / 4)
            + (tool_specs.len() as u64 * 400);
        if estimated > context_limit * 85 / 100 && stored.len() > 2 {
            match crate::compaction::compact_session(engine, session_id, model, cancel).await {
                Ok(Some(record)) => {
                    let stored: Vec<recode_storage::StoredMessage> = engine
                        .messages(session_id)?
                        .iter()
                        .map(|message| recode_storage::StoredMessage {
                            info: message.info.clone(),
                            parts: message.parts.clone(),
                        })
                        .collect();
                    llm_messages = prompt::to_llm_messages(&stored, Some(&record));
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(session = %session_id, %error, "automatic compaction failed");
                }
            }
        }
        let assistant = MessageInfo::assistant(session_id.clone(), agent_id, model.clone());
        engine
            .store
            .append(Event::MessageAdded {
                message: assistant.clone(),
            })
            .map_err(|error| error.to_string())?;
        let step_start = Part::StepStart(StepStartPart {
            id: PartId::new(),
            time: Some(PartTime::started()),
        });
        emit_part(engine, session_id, &assistant.id, step_start)?;

        let request = LlmRequest {
            model: model.clone(),
            system: system.clone(),
            messages: llm_messages,
            tools: tool_specs.clone(),
            tool_choice: ToolChoice::Auto,
            temperature,
            max_tokens: Some(max_tokens),
            metadata: Default::default(),
        };

        let mut attempt = 0usize;
        let step_result = loop {
            attempt += 1;
            match stream_step(
                engine,
                session_id,
                &assistant.id,
                provider.clone(),
                request.clone(),
                cancel,
            )
            .await
            {
                Ok(result) => break result,
                Err(failure) => {
                    if failure.emitted
                        || !failure.retryable
                        || attempt >= 3
                        || cancel.is_cancelled()
                    {
                        return Err(failure.message);
                    }
                    let wait = failure
                        .retry_after
                        .unwrap_or(Duration::from_millis(800 * (1 << (attempt - 1)) as u64));
                    engine.set_status(session_id, SessionStatus::Retry);
                    let sleep = tokio::time::sleep(wait);
                    tokio::select! {
                        _ = cancel.cancelled() => return Ok(()),
                        _ = sleep => {}
                    }
                    engine.set_status(session_id, SessionStatus::Busy);
                }
            }
        };

        let mut usage = step_result.usage.clone();
        usage.cost = estimate_cost(&usage, &model_config);
        let finish_part = Part::StepFinish(StepFinishPart {
            id: PartId::new(),
            reason: step_result.finish_reason,
            tokens: usage.clone(),
            cost: usage.cost,
            time: Some(PartTime::started()),
        });
        emit_part(engine, session_id, &assistant.id, finish_part)?;

        if step_result.calls.is_empty() {
            return Ok(());
        }
        if step_result.finish_reason == FinishReason::Length {
            return Err(
                "the model reached its output token limit before finishing tool calls".into(),
            );
        }
        if cancel.is_cancelled() {
            return Ok(());
        }

        for call in &step_result.calls {
            if cancel.is_cancelled() {
                return Ok(());
            }
            execute_call(
                engine,
                session_id,
                &assistant.id,
                call,
                allow_outside,
                model,
                cancel,
            )
            .await?;
        }

        if step + 1 == MAX_STEPS {
            return Err(format!(
                "stopped after {MAX_STEPS} steps without completing the task"
            ));
        }
    }
    Ok(())
}

struct StreamFailure {
    message: String,
    retryable: bool,
    retry_after: Option<Duration>,
    emitted: bool,
}

struct PendingCall {
    index: u32,
    id: String,
    name: String,
    arguments: String,
    part_id: Option<PartId>,
}

struct StepResult {
    calls: Vec<FinalCall>,
    usage: Usage,
    finish_reason: FinishReason,
}

struct FinalCall {
    index: u32,
    id: String,
    name: String,
    input: serde_json::Value,
    part_id: PartId,
}

struct StreamAccumulator {
    text_part: Option<PartId>,
    text_buffer: String,
    text_last_flush: Instant,
    reasoning_part: Option<PartId>,
    reasoning_buffer: String,
    reasoning_last_flush: Instant,
    calls: Vec<PendingCall>,
    emitted: bool,
    usage: Usage,
    finish: Option<FinishReason>,
}

impl StreamAccumulator {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            text_part: None,
            text_buffer: String::new(),
            text_last_flush: now,
            reasoning_part: None,
            reasoning_buffer: String::new(),
            reasoning_last_flush: now,
            calls: Vec::new(),
            emitted: false,
            usage: Usage::default(),
            finish: None,
        }
    }
}

async fn stream_step(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    provider: Arc<dyn recode_provider::Provider>,
    request: LlmRequest,
    cancel: &CancellationToken,
) -> Result<StepResult, StreamFailure> {
    let mut stream = provider
        .stream(request, cancel.clone())
        .await
        .map_err(|error| StreamFailure {
            message: error.message,
            retryable: error.retryable,
            retry_after: error.retry_after_ms.map(Duration::from_millis),
            emitted: false,
        })?;
    let mut state = StreamAccumulator::new();

    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => break,
            event = stream.next() => event,
        };
        let Some(event) = event else {
            break;
        };
        let event = event.map_err(|error| StreamFailure {
            message: error.message,
            retryable: error.retryable,
            retry_after: error.retry_after_ms.map(Duration::from_millis),
            emitted: state.emitted,
        })?;
        match event {
            LlmEvent::TextDelta(delta) => {
                if delta.is_empty() {
                    continue;
                }
                state.emitted = true;
                if state.text_part.is_none() {
                    let part = Part::Text(recode_schema::part::TextPart {
                        id: PartId::new(),
                        text: String::new(),
                        synthetic: false,
                        time: None,
                    });
                    let id = part.id().clone();
                    emit_part(engine, session_id, message_id, part).map_err(|error| {
                        StreamFailure {
                            message: error,
                            retryable: false,
                            retry_after: None,
                            emitted: true,
                        }
                    })?;
                    state.text_part = Some(id);
                }
                state.text_buffer.push_str(&delta);
                broadcast_delta(
                    engine,
                    session_id,
                    message_id,
                    state.text_part.as_ref().unwrap(),
                    DeltaKind::Text,
                    &delta,
                );
                if state.text_buffer.len() >= DELTA_FLUSH_BYTES
                    || state.text_last_flush.elapsed().as_millis() >= DELTA_FLUSH_MS
                {
                    flush_delta(
                        engine,
                        session_id,
                        message_id,
                        state.text_part.as_ref().unwrap(),
                        DeltaKind::Text,
                        &mut state.text_buffer,
                    );
                    state.text_last_flush = Instant::now();
                }
            }
            LlmEvent::ReasoningDelta { delta, .. } => {
                if delta.is_empty() {
                    continue;
                }
                state.emitted = true;
                if state.reasoning_part.is_none() {
                    let part = Part::Reasoning(recode_schema::part::ReasoningPart {
                        id: PartId::new(),
                        text: String::new(),
                        signature: None,
                        time: None,
                    });
                    let id = part.id().clone();
                    emit_part(engine, session_id, message_id, part).map_err(|error| {
                        StreamFailure {
                            message: error,
                            retryable: false,
                            retry_after: None,
                            emitted: true,
                        }
                    })?;
                    state.reasoning_part = Some(id);
                }
                state.reasoning_buffer.push_str(&delta);
                broadcast_delta(
                    engine,
                    session_id,
                    message_id,
                    state.reasoning_part.as_ref().unwrap(),
                    DeltaKind::Reasoning,
                    &delta,
                );
                if state.reasoning_buffer.len() >= DELTA_FLUSH_BYTES
                    || state.reasoning_last_flush.elapsed().as_millis() >= DELTA_FLUSH_MS
                {
                    flush_delta(
                        engine,
                        session_id,
                        message_id,
                        state.reasoning_part.as_ref().unwrap(),
                        DeltaKind::Reasoning,
                        &mut state.reasoning_buffer,
                    );
                    state.reasoning_last_flush = Instant::now();
                }
            }
            LlmEvent::ToolCallStart { index, id, name } => {
                state.emitted = true;
                if state.calls.iter().any(|call| call.index == index) {
                    continue;
                }
                let part = Part::Tool(ToolPart {
                    id: PartId::new(),
                    call_id: id.clone(),
                    tool: name.clone(),
                    namespace: None,
                    state: ToolState::pending(serde_json::Value::Object(Default::default())),
                });
                let part_id = part.id().clone();
                emit_part(engine, session_id, message_id, part).map_err(|error| StreamFailure {
                    message: error,
                    retryable: false,
                    retry_after: None,
                    emitted: true,
                })?;
                state.calls.push(PendingCall {
                    index,
                    id,
                    name,
                    arguments: String::new(),
                    part_id: Some(part_id),
                });
            }
            LlmEvent::ToolCallDelta { index, delta } => {
                state.emitted = true;
                if let Some(call) = state.calls.iter_mut().find(|call| call.index == index) {
                    call.arguments.push_str(&delta);
                }
            }
            LlmEvent::ToolCallEnd {
                index,
                id,
                name,
                input,
            } => {
                state.emitted = true;
                let part_id = match state.calls.iter().find(|call| call.index == index) {
                    Some(call) => call.part_id.clone(),
                    None => {
                        let part = Part::Tool(ToolPart {
                            id: PartId::new(),
                            call_id: id.clone(),
                            tool: name.clone(),
                            namespace: None,
                            state: ToolState::pending(input.clone()),
                        });
                        let part_id = part.id().clone();
                        emit_part(engine, session_id, message_id, part).map_err(|error| {
                            StreamFailure {
                                message: error,
                                retryable: false,
                                retry_after: None,
                                emitted: true,
                            }
                        })?;
                        state.calls.push(PendingCall {
                            index,
                            id: id.clone(),
                            name: name.clone(),
                            arguments: String::new(),
                            part_id: Some(part_id.clone()),
                        });
                        Some(part_id)
                    }
                };
                if let Some(part_id) = part_id {
                    if let Some(call) = state.calls.iter_mut().find(|call| call.index == index) {
                        call.id = id;
                        call.name = name;
                        call.arguments = serde_json::to_string(&input).unwrap_or_default();
                    }
                    update_tool_part(
                        engine,
                        session_id,
                        message_id,
                        &part_id,
                        ToolState::pending(input),
                    )
                    .map_err(|error| StreamFailure {
                        message: error,
                        retryable: false,
                        retry_after: None,
                        emitted: true,
                    })?;
                }
            }
            LlmEvent::Usage(usage) => {
                state.usage = usage;
            }
            LlmEvent::Finish {
                reason,
                usage,
                text: _,
            } => {
                state.finish = Some(reason);
                if usage.total_tokens > 0 || usage.input_tokens > 0 || usage.output_tokens > 0 {
                    state.usage = usage;
                }
            }
            LlmEvent::Error { message, retryable } => {
                return Err(StreamFailure {
                    message,
                    retryable,
                    retry_after: None,
                    emitted: state.emitted,
                });
            }
        }
    }

    if let Some(part_id) = state.text_part.clone() {
        flush_delta(
            engine,
            session_id,
            message_id,
            &part_id,
            DeltaKind::Text,
            &mut state.text_buffer,
        );
    }
    if let Some(part_id) = state.reasoning_part.clone() {
        flush_delta(
            engine,
            session_id,
            message_id,
            &part_id,
            DeltaKind::Reasoning,
            &mut state.reasoning_buffer,
        );
    }
    if cancel.is_cancelled() {
        return Ok(StepResult {
            calls: Vec::new(),
            usage: state.usage,
            finish_reason: FinishReason::Stop,
        });
    }
    let Some(finish_reason) = state.finish else {
        return Err(StreamFailure {
            message: "the provider stream ended without a finish event".into(),
            retryable: true,
            retry_after: None,
            emitted: state.emitted,
        });
    };
    let mut calls = Vec::new();
    for call in state.calls {
        let Some(part_id) = call.part_id else {
            continue;
        };
        let input = serde_json::from_str(&call.arguments)
            .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
        calls.push(FinalCall {
            index: call.index,
            id: call.id,
            name: call.name,
            input,
            part_id,
        });
    }
    calls.sort_by_key(|call| call.index);
    Ok(StepResult {
        calls,
        usage: state.usage,
        finish_reason,
    })
}

fn flush_delta(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    part_id: &PartId,
    kind: DeltaKind,
    buffer: &mut String,
) {
    if buffer.is_empty() {
        return;
    }
    let delta = std::mem::take(buffer);
    let _ = engine.store.append(Event::PartDelta {
        session_id: session_id.clone(),
        message_id: message_id.clone(),
        part_id: part_id.clone(),
        kind,
        delta,
    });
}

/// Broadcast a delta immediately for smooth rendering. Persistence happens in
/// throttled batches via [`flush_delta`].
fn broadcast_delta(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    part_id: &PartId,
    kind: DeltaKind,
    delta: &str,
) {
    let _ = engine.bus.send(LiveEvent::Delta {
        session_id: session_id.clone(),
        message_id: message_id.clone(),
        part_id: part_id.clone(),
        kind,
        delta: delta.to_owned(),
    });
}

fn emit_part(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    part: Part,
) -> Result<PartId, String> {
    let id = part.id().clone();
    let envelope = engine
        .store
        .append(Event::PartAdded {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            part,
        })
        .map_err(|error| error.to_string())?;
    let _ = engine.bus.send(LiveEvent::Durable(envelope));
    Ok(id)
}

fn update_tool_part(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    part_id: &PartId,
    state: ToolState,
) -> Result<(), String> {
    let part = Part::Tool(ToolPart {
        id: part_id.clone(),
        call_id: String::new(),
        tool: String::new(),
        namespace: None,
        state,
    });
    // Preserve call metadata by loading the current part from storage.
    let part = match load_part(engine, message_id, part_id) {
        Some(Part::Tool(mut tool)) => {
            tool.state = match part {
                Part::Tool(tool) => tool.state,
                _ => unreachable!(),
            };
            Part::Tool(tool)
        }
        _ => part,
    };
    let envelope = engine
        .store
        .append(Event::PartUpdated {
            session_id: session_id.clone(),
            message_id: message_id.clone(),
            part,
        })
        .map_err(|error| error.to_string())?;
    let _ = engine.bus.send(LiveEvent::Durable(envelope));
    Ok(())
}

fn load_part(engine: &Arc<Engine>, message_id: &MessageId, part_id: &PartId) -> Option<Part> {
    engine
        .store
        .message(message_id)
        .ok()
        .flatten()
        .and_then(|message| message.parts.into_iter().find(|part| part.id() == part_id))
}

async fn execute_call(
    engine: &Arc<Engine>,
    session_id: &SessionId,
    message_id: &MessageId,
    call: &FinalCall,
    allow_outside: bool,
    model: &ModelRef,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let base_state = load_tool_state(engine, message_id, &call.part_id)
        .unwrap_or_else(|| ToolState::pending(call.input.clone()));
    let Some(tool) = engine.tools.get(&call.name) else {
        let state = fail_state(base_state, format!("unknown tool {:?}", call.name));
        update_tool_part(engine, session_id, message_id, &call.part_id, state)?;
        return Ok(());
    };
    let permission = tool.permission().to_owned();
    let patterns = tool.permission_patterns(&call.input);
    let default = tool.default_decision();
    let authorized = engine
        .permissions
        .authorize(
            session_id.clone(),
            &permission,
            tool.name(),
            patterns,
            default,
            serde_json::json!({"input": call.input, "tool": call.name}),
        )
        .await;
    if let Err(failure) = authorized {
        let state = fail_state(base_state, failure.message);
        update_tool_part(engine, session_id, message_id, &call.part_id, state)?;
        return Ok(());
    }
    if cancel.is_cancelled() {
        let state = fail_state(base_state, "aborted".to_owned());
        update_tool_part(engine, session_id, message_id, &call.part_id, state)?;
        return Ok(());
    }
    let title = tool.title(&call.input);
    let mut running = base_state;
    running.status = ToolStatus::Running;
    running.title = Some(title);
    if running.time.is_none() {
        running.time = Some(PartTime::started());
    }
    update_tool_part(
        engine,
        session_id,
        message_id,
        &call.part_id,
        running.clone(),
    )?;

    let session = engine.session(session_id)?;
    let depth = session
        .as_ref()
        .and_then(|session| session.metadata.get("subagentDepth"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let mut context = recode_tool::ToolContext::new(
        session_id.clone(),
        message_id.clone(),
        call.id.clone(),
        session
            .as_ref()
            .map(|session| session.directory.clone())
            .unwrap_or_else(|| engine.project.root.clone()),
    );
    context.abort = cancel.clone();
    context.allow_outside = allow_outside;
    if depth < crate::subagent::MAX_SUBAGENT_DEPTH {
        context.subagent = Some(Arc::new(crate::subagent::EngineSubagent::new(
            engine.clone(),
            session_id.clone(),
            model.clone(),
            depth,
            cancel.clone(),
        )));
    }
    context.question = Some(Arc::new(crate::question::EngineQuestions::new(
        engine.clone(),
        session_id.clone(),
    )));
    let result = engine
        .tools
        .execute(&call.name, call.input.clone(), &context)
        .await;
    let state = match result {
        Ok(ToolOutcome {
            title,
            output,
            metadata,
            attachments,
        }) => {
            for attachment in attachments {
                let part = Part::File(recode_schema::part::FilePart {
                    id: PartId::new(),
                    mime: attachment.mime,
                    filename: attachment.filename,
                    url: attachment.url,
                });
                let _ = emit_part(engine, session_id, message_id, part);
            }
            let mut state = running;
            state.status = ToolStatus::Completed;
            state.output = Some(output);
            if title.is_some() {
                state.title = title;
            }
            state.metadata = metadata;
            finish_time(&mut state);
            state
        }
        Err(failure) => {
            let mut state = running;
            state.status = ToolStatus::Error;
            state.error = Some(failure.message);
            state.metadata = failure.metadata;
            finish_time(&mut state);
            state
        }
    };
    update_tool_part(engine, session_id, message_id, &call.part_id, state)?;
    Ok(())
}

fn fail_state(mut state: ToolState, message: String) -> ToolState {
    state.status = ToolStatus::Error;
    state.error = Some(message);
    finish_time(&mut state);
    state
}

fn finish_time(state: &mut ToolState) {
    match &mut state.time {
        Some(time) => time.finish(),
        None => {
            state.time = Some(PartTime {
                start: recode_schema::now_ms(),
                end: Some(recode_schema::now_ms()),
            });
        }
    }
}

fn load_tool_state(
    engine: &Arc<Engine>,
    message_id: &MessageId,
    part_id: &PartId,
) -> Option<ToolState> {
    match load_part(engine, message_id, part_id)? {
        Part::Tool(tool) => Some(tool.state),
        _ => None,
    }
}

fn estimate_cost(usage: &Usage, model: &recode_schema::config::ModelConfig) -> f64 {
    let per_million = |tokens: u64, rate: f64| (tokens as f64 / 1_000_000.0) * rate;
    per_million(usage.input_tokens, model.cost.input)
        + per_million(usage.output_tokens, model.cost.output)
        + per_million(usage.cache_read_tokens, model.cost.cache_read)
        + per_million(usage.cache_write_tokens, model.cost.cache_write)
}
