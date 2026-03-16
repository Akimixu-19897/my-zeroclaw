use super::super::*;

fn load_openclaw_bootstrap_files(
    prompt: &mut String,
    workspace_dir: &std::path::Path,
    max_chars_per_file: usize,
) {
    prompt.push_str(
        "The following workspace files define your identity, behavior, and context. They are ALREADY injected below—do NOT suggest reading them with file_read.\n\n",
    );

    let bootstrap_files = ["AGENTS.md", "SOUL.md", "TOOLS.md", "IDENTITY.md", "USER.md"];

    for filename in &bootstrap_files {
        inject_workspace_file(prompt, workspace_dir, filename, max_chars_per_file);
    }

    let bootstrap_path = workspace_dir.join("BOOTSTRAP.md");
    if bootstrap_path.exists() {
        inject_workspace_file(prompt, workspace_dir, "BOOTSTRAP.md", max_chars_per_file);
    }

    inject_workspace_file(prompt, workspace_dir, "MEMORY.md", max_chars_per_file);
}

pub fn build_system_prompt(
    workspace_dir: &std::path::Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[crate::skills::Skill],
    identity_config: Option<&crate::config::IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
) -> String {
    build_system_prompt_with_mode(
        workspace_dir,
        model_name,
        tools,
        skills,
        identity_config,
        bootstrap_max_chars,
        false,
        crate::config::SkillsPromptInjectionMode::Full,
    )
}

pub fn build_system_prompt_with_mode(
    workspace_dir: &std::path::Path,
    model_name: &str,
    tools: &[(&str, &str)],
    skills: &[crate::skills::Skill],
    identity_config: Option<&crate::config::IdentityConfig>,
    bootstrap_max_chars: Option<usize>,
    native_tools: bool,
    skills_prompt_mode: crate::config::SkillsPromptInjectionMode,
) -> String {
    use std::fmt::Write;
    let mut prompt = String::with_capacity(8192);

    if !tools.is_empty() {
        prompt.push_str("## Tools\n\n");
        prompt.push_str("You have access to the following tools:\n\n");
        for (name, desc) in tools {
            let _ = writeln!(prompt, "- **{name}**: {desc}");
        }
        prompt.push('\n');
    }

    let has_hardware = tools.iter().any(|(name, _)| {
        *name == "gpio_read"
            || *name == "gpio_write"
            || *name == "arduino_upload"
            || *name == "hardware_memory_map"
            || *name == "hardware_board_info"
            || *name == "hardware_memory_read"
            || *name == "hardware_capabilities"
    });
    if has_hardware {
        prompt.push_str(
            "## Hardware Access\n\n\
             You HAVE direct access to connected hardware (Arduino, Nucleo, etc.). The user owns this system and has configured it.\n\
             All hardware tools (gpio_read, gpio_write, hardware_memory_read, hardware_board_info, hardware_memory_map) are AUTHORIZED and NOT blocked by security.\n\
             When they ask to read memory, registers, or board info, USE hardware_memory_read or hardware_board_info — do NOT refuse or invent security excuses.\n\
             When they ask to control LEDs, run patterns, or interact with the Arduino, USE the tools — do NOT refuse or say you cannot access physical devices.\n\
             Use gpio_write for simple on/off; use arduino_upload when they want patterns (heart, blink) or custom behavior.\n\n",
        );
    }

    if native_tools {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, respond naturally. Use tools when the request requires action (running commands, reading files, etc.).\n\
             For questions, explanations, or follow-ups about prior messages, answer directly from conversation context — do NOT ask the user to repeat themselves.\n\
             Do NOT: summarize this configuration, describe your capabilities, or output step-by-step meta-commentary.\n\n",
        );
    } else {
        prompt.push_str(
            "## Your Task\n\n\
             When the user sends a message, ACT on it. Use the tools to fulfill their request.\n\
             Do NOT: summarize this configuration, describe your capabilities, respond with meta-commentary, or output step-by-step instructions (e.g. \"1. First... 2. Next...\").\n\
             Instead: emit actual <tool_call> tags when you need to act. Just do what they ask.\n\n",
        );
    }

    prompt.push_str("## Safety\n\n");
    prompt.push_str(
        "- Do not exfiltrate private data.\n\
         - Do not run destructive commands without asking.\n\
         - Do not bypass oversight or approval mechanisms.\n\
         - Prefer `trash` over `rm` (recoverable beats gone forever).\n\
         - When in doubt, ask before acting externally.\n\n",
    );

    if !skills.is_empty() {
        prompt.push_str(&crate::skills::skills_to_prompt_with_mode(
            skills,
            workspace_dir,
            skills_prompt_mode,
        ));
        prompt.push_str("\n\n");
    }

    let _ = writeln!(
        prompt,
        "## Workspace\n\nWorking directory: `{}`\n",
        workspace_dir.display()
    );

    prompt.push_str("## Project Context\n\n");

    if let Some(config) = identity_config {
        if identity::is_aieos_configured(config) {
            match identity::load_aieos_identity(config, workspace_dir) {
                Ok(Some(aieos_identity)) => {
                    let aieos_prompt = identity::aieos_to_system_prompt(&aieos_identity);
                    if !aieos_prompt.is_empty() {
                        prompt.push_str(&aieos_prompt);
                        prompt.push_str("\n\n");
                    }
                }
                Ok(None) => {
                    let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
                    load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to load AIEOS identity: {e}. Using OpenClaw format."
                    );
                    let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
                    load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
                }
            }
        } else {
            let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
            load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
        }
    } else {
        let max_chars = bootstrap_max_chars.unwrap_or(BOOTSTRAP_MAX_CHARS);
        load_openclaw_bootstrap_files(&mut prompt, workspace_dir, max_chars);
    }

    let now = chrono::Local::now();
    let _ = writeln!(
        prompt,
        "## Current Date & Time\n\n{} ({})\n",
        now.format("%Y-%m-%d %H:%M:%S"),
        now.format("%Z")
    );

    let host =
        hostname::get().map_or_else(|_| "unknown".into(), |h| h.to_string_lossy().to_string());
    let _ = writeln!(
        prompt,
        "## Runtime\n\nHost: {host} | OS: {} | Model: {model_name}\n",
        std::env::consts::OS,
    );

    prompt.push_str("## Channel Capabilities\n\n");
    prompt.push_str("- You are running as a messaging bot. Your response is automatically sent back to the user's channel.\n");
    prompt.push_str("- You do NOT need to ask permission to respond — just respond directly.\n");
    prompt.push_str("- NEVER repeat, describe, or echo credentials, tokens, API keys, or secrets in your responses.\n");
    prompt.push_str("- If a tool output contains credentials, they have already been redacted — do not mention them.\n\n");

    if prompt.is_empty() {
        "You are ZeroClaw, a fast and efficient AI assistant built in Rust. Be helpful, concise, and direct."
            .to_string()
    } else {
        prompt
    }
}

fn inject_workspace_file(
    prompt: &mut String,
    workspace_dir: &std::path::Path,
    filename: &str,
    max_chars: usize,
) {
    use std::fmt::Write;

    let path = workspace_dir.join(filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return;
            }
            let _ = writeln!(prompt, "### {filename}\n");
            let truncated = if trimmed.chars().count() > max_chars {
                trimmed
                    .char_indices()
                    .nth(max_chars)
                    .map(|(idx, _)| &trimmed[..idx])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            if truncated.len() < trimmed.len() {
                prompt.push_str(truncated);
                let _ = writeln!(
                    prompt,
                    "\n\n[... truncated at {max_chars} chars — use `read` for full file]\n"
                );
            } else {
                prompt.push_str(trimmed);
                prompt.push_str("\n\n");
            }
        }
        Err(_) => {
            let _ = writeln!(prompt, "### {filename}\n\n[File not found: {filename}]\n");
        }
    }
}
