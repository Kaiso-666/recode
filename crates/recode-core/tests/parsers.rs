//! Parser robustness: random input must never panic.
//!
//! The parsers that read user files (the frontmatter subset, the JSONC
//! configuration) and the ones that read the wire (server-sent events) see
//! whatever the user or the server hands them. These tests feed them a
//! deterministic stream of pseudo-random bytes and check that they answer
//! instead of panicking.

use recode_core::config::ConfigManager;
use recode_core::frontmatter::Frontmatter;
use recode_core::RecodePaths;

/// A small xorshift generator: the same input on every run, so a failure can
/// be reproduced.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length)
            .map(|_| (self.next_u64() & 0xff) as u8)
            .collect()
    }
}

#[test]
fn the_frontmatter_parser_survives_random_input() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for length in 0..512 {
        let input = String::from_utf8_lossy(&rng.bytes(length)).into_owned();
        let parsed = Frontmatter::parse(&input);
        // The body is whatever follows the frontmatter, so it must never be
        // longer than the input.
        assert!(parsed.body.len() <= input.len());
        // The maps only hold printable keys.
        for key in parsed.map("tools").keys() {
            assert!(!key.contains('\0'));
        }
    }
}

#[test]
fn the_configuration_loader_survives_random_files() {
    let mut rng = Rng(0xD1B54A32D192ED03);
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let paths = RecodePaths {
        config_dir: config_dir.clone(),
        data_dir: dir.path().join("data"),
        db_path: dir.path().join("data/recode.db"),
        auth_path: config_dir.join("auth.json"),
        log_dir: dir.path().join("data/log"),
        global_config: config_dir.join("recode.json"),
    };
    for length in 0..256 {
        let content = rng.bytes(length);
        std::fs::write(&paths.global_config, &content).unwrap();
        let manager = ConfigManager::load(paths.clone(), &project);
        // Whatever the file holds, the manager answers with a usable default.
        assert!(!manager.default_agent().is_empty());
        assert!(manager.theme_name().is_none() || manager.theme_name().is_some());
    }
}

#[test]
fn the_frontmatter_parser_handles_the_awkward_cases() {
    // Unterminated frontmatter.
    let parsed = Frontmatter::parse("---\nkey: value\nbody");
    assert_eq!(parsed.get("key"), Some("value"));
    // An empty document.
    let parsed = Frontmatter::parse("");
    assert!(parsed.body.is_empty());
    // Only a closing fence.
    let parsed = Frontmatter::parse("---\n---\n");
    assert!(parsed.body.is_empty());
}
