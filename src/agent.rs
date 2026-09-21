use std::io::IsTerminal;

const AGENT_ENV_VARS: &[&str] = &[
    "MASKRUN_AGENT",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "AI_AGENT",
    "AIDER_CHAT",
    "CURSOR_AGENT",
    "OPENAI_CODEX",
    "GEMINI_CLI",
    "REPLIT_AGENT",
];

fn env_is_set(var: &str) -> bool {
    std::env::var(var).map(|v| !v.is_empty()).unwrap_or(false)
}

pub fn in_agent() -> bool {
    if let Ok(flag) = std::env::var("MASKRUN_AGENT") {
        if matches!(flag.as_str(), "0" | "false" | "no") {
            return false;
        }
    }
    AGENT_ENV_VARS.iter().any(|var| env_is_set(var))
}

// Not called yet — run.rs/mask.rs wire this in.
#[allow(dead_code)]
pub fn should_mask(raw: bool, force: bool) -> bool {
    if force {
        return true;
    }
    if raw {
        return false;
    }
    if let Ok(flag) = std::env::var("MASKRUN_MASK") {
        if matches!(flag.as_str(), "0" | "false" | "no") {
            return false;
        }
        if matches!(flag.as_str(), "1" | "true" | "yes") {
            return true;
        }
    }
    if in_agent() {
        return true;
    }
    !std::io::stdout().is_terminal()
}

pub fn refuse_in_agent(command: &str, alternative: &str) -> crate::error::Result<()> {
    let allowed = std::env::var("MASKRUN_ALLOW_READ")
        .map(|v| v == "1")
        .unwrap_or(false);
    if in_agent() && !allowed {
        return Err(crate::error::Error::msg(format!(
            "'{command}' is disabled inside an AI agent session — the value would go \
             straight into the transcript.\n{alternative}\n\
             If you need the value itself (rotating a key, pasting it into a \
             dashboard), run this in your own terminal."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; serialize so tests don't clobber each
    // other under cargo test's default multi-threaded runner.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_agent_env() {
        for var in AGENT_ENV_VARS {
            std::env::remove_var(var);
        }
        std::env::remove_var("MASKRUN_MASK");
        std::env::remove_var("MASKRUN_ALLOW_READ");
    }

    #[test]
    fn not_in_agent_by_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        assert!(!in_agent());
    }

    #[test]
    fn agent_env_var_detected() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("CLAUDECODE", "1");
        assert!(in_agent());
        clear_agent_env();
    }

    #[test]
    fn maskrun_agent_0_overrides_detection() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("MASKRUN_AGENT", "0");
        assert!(!in_agent());
        clear_agent_env();
    }

    #[test]
    fn refuse_in_agent_blocks_when_in_agent() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("MASKRUN_AGENT", "1");
        let err = refuse_in_agent("maskrun get", "alt").unwrap_err();
        assert!(err
            .to_string()
            .contains("disabled inside an AI agent session"));
        clear_agent_env();
    }

    #[test]
    fn refuse_in_agent_allows_with_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("MASKRUN_AGENT", "1");
        std::env::set_var("MASKRUN_ALLOW_READ", "1");
        assert!(refuse_in_agent("maskrun get", "alt").is_ok());
        clear_agent_env();
    }

    #[test]
    fn should_mask_force_wins() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        assert!(should_mask(true, true));
    }

    #[test]
    fn should_mask_raw_disables() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("MASKRUN_AGENT", "1");
        assert!(!should_mask(true, false));
        clear_agent_env();
    }

    #[test]
    fn should_mask_env_var_zero_disables() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_agent_env();
        std::env::set_var("MASKRUN_AGENT", "1");
        std::env::set_var("MASKRUN_MASK", "0");
        assert!(!should_mask(false, false));
        clear_agent_env();
    }
}
