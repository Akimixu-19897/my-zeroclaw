use super::super::*;

pub(crate) fn channel_delivery_instructions(channel_name: &str) -> Option<&'static str> {
    let normalized = channel_name.split(':').next().unwrap_or(channel_name);
    match normalized {
        "matrix" => Some(
            "When responding on Matrix:\n\
             - Use Markdown formatting (bold, italic, code blocks)\n\
             - Be concise and direct\n\
             - When you receive a [Voice message], the user spoke to you. Respond naturally as in conversation.\n\
             - Your text reply will automatically be converted to audio and sent back as a voice message.\n",
        ),
        "telegram" => Some(
            "When responding on Telegram:\n\
             - Include media markers for files or URLs that should be sent as attachments\n\
             - Use **bold** for key terms, section titles, and important info (renders as <b>)\n\
             - Use *italic* for emphasis (renders as <i>)\n\
             - Use `backticks` for inline code, commands, or technical terms\n\
             - Use triple backticks for code blocks\n\
             - Use emoji naturally to add personality — but don't overdo it\n\
             - Be concise and direct. Skip filler phrases like 'Great question!' or 'Certainly!'\n\
             - Structure longer answers with bold headers, not raw markdown ## headers\n\
             - For media attachments use markers: [IMAGE:<path-or-url>], [DOCUMENT:<path-or-url>], [VIDEO:<path-or-url>], [AUDIO:<path-or-url>], or [VOICE:<path-or-url>]\n\
             - Keep normal text outside markers and never wrap markers in code fences.\n\
             - Use tool results silently: answer the latest user message directly, and do not narrate delayed/internal tool execution bookkeeping.",
        ),
        "feishu" => Some(
            "When responding on Feishu:\n\
             - Use channel attachment markers instead of writing one-off upload scripts.\n\
             - Do not write ad-hoc upload scripts, temp automation glue, or direct HTTP upload code when a channel marker can express the attachment.\n\
             - For images use [IMAGE:<absolute-path-or-url>].\n\
             - For files use [DOCUMENT:<absolute-path>] or [FILE:<absolute-path>].\n\
             - Keep explanatory text outside markers and never wrap markers in code fences.\n\
             - If a local file already exists in the workspace or on disk, reference it directly with a marker instead of re-uploading it manually from a tool.\n\
             - Use tool results silently: answer the latest user message directly, and do not narrate delayed/internal tool execution bookkeeping.",
        ),
        "lark" => Some(
            "When responding on Lark:\n\
             - Use channel attachment markers instead of writing one-off upload scripts.\n\
             - Do not write ad-hoc upload scripts, temp automation glue, or direct HTTP upload code when a channel marker can express the attachment.\n\
             - For images use [IMAGE:<absolute-path-or-url>].\n\
             - For files use [DOCUMENT:<absolute-path>] or [FILE:<absolute-path>].\n\
             - Keep explanatory text outside markers and never wrap markers in code fences.\n\
             - If a local file already exists in the workspace or on disk, reference it directly with a marker instead of re-uploading it manually from a tool.\n\
             - Use tool results silently: answer the latest user message directly, and do not narrate delayed/internal tool execution bookkeeping.",
        ),
        "wecom" => Some(
            "When responding on WeCom:\n\
             - Use channel attachment markers instead of writing one-off upload scripts.\n\
             - Do not write ad-hoc upload scripts, temp automation glue, or direct HTTP upload code when a channel marker can express the attachment.\n\
             - For images use [IMAGE:<absolute-path-or-url>].\n\
             - Keep explanatory text outside markers and never wrap markers in code fences.\n\
             - Use tool results silently: answer the latest user message directly, and do not narrate delayed/internal tool execution bookkeeping.",
        ),
        _ => None,
    }
}

pub(crate) fn build_channel_system_prompt(
    base_prompt: &str,
    channel_name: &str,
    reply_target: &str,
) -> String {
    let mut prompt = base_prompt.to_string();

    // Refresh the stale datetime in the cached system prompt
    {
        let now = chrono::Local::now();
        let fresh = format!(
            "## Current Date & Time\n\n{} ({})\n",
            now.format("%Y-%m-%d %H:%M:%S"),
            now.format("%Z"),
        );
        if let Some(start) = prompt.find("## Current Date & Time\n\n") {
            // Find the end of this section (next "## " heading or end of string)
            let rest = &prompt[start + 24..]; // skip past "## Current Date & Time\n\n"
            let section_end = rest
                .find("\n## ")
                .map(|i| start + 24 + i)
                .unwrap_or(prompt.len());
            prompt.replace_range(start..section_end, fresh.trim_end());
        }
    }

    if let Some(instructions) = channel_delivery_instructions(channel_name) {
        if prompt.is_empty() {
            prompt = instructions.to_string();
        } else {
            prompt = format!("{prompt}\n\n{instructions}");
        }
    }

    if !reply_target.is_empty() {
        let context = format!(
            "\n\nChannel context: You are currently responding on channel={channel_name}, \
             reply_target={reply_target}. When scheduling delayed messages or reminders \
             via cron_add for this conversation, use delivery={{\"mode\":\"announce\",\
             \"channel\":\"{channel_name}\",\"to\":\"{reply_target}\"}} so the message \
             reaches the user."
        );
        prompt.push_str(&context);
    }

    prompt
}

pub(crate) fn normalize_cached_channel_turns(turns: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut normalized = Vec::with_capacity(turns.len());
    let mut expecting_user = true;

    for turn in turns {
        match (expecting_user, turn.role.as_str()) {
            (true, "user") => {
                normalized.push(turn);
                expecting_user = false;
            }
            (false, "assistant") => {
                normalized.push(turn);
                expecting_user = true;
            }
            // Interrupted channel turns can produce consecutive user messages
            // (no assistant persisted yet). Merge instead of dropping.
            (false, "user") | (true, "assistant") => {
                if let Some(last_turn) = normalized.last_mut() {
                    if !turn.content.is_empty() {
                        if !last_turn.content.is_empty() {
                            last_turn.content.push_str("\n\n");
                        }
                        last_turn.content.push_str(&turn.content);
                    }
                }
            }
            _ => {}
        }
    }

    normalized
}

pub(crate) fn supports_runtime_model_switch(channel_name: &str) -> bool {
    matches!(channel_name, "telegram" | "discord" | "matrix")
}

pub(crate) fn parse_runtime_command(
    channel_name: &str,
    content: &str,
) -> Option<ChannelRuntimeCommand> {
    if !supports_runtime_model_switch(channel_name) {
        return None;
    }

    let trimmed = content.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let mut parts = trimmed.split_whitespace();
    let command_token = parts.next()?;
    let base_command = command_token
        .split('@')
        .next()
        .unwrap_or(command_token)
        .to_ascii_lowercase();

    match base_command.as_str() {
        "/models" => {
            if let Some(provider) = parts.next() {
                Some(ChannelRuntimeCommand::SetProvider(
                    provider.trim().to_string(),
                ))
            } else {
                Some(ChannelRuntimeCommand::ShowProviders)
            }
        }
        "/model" => {
            let model = parts.collect::<Vec<_>>().join(" ").trim().to_string();
            if model.is_empty() {
                Some(ChannelRuntimeCommand::ShowModel)
            } else {
                Some(ChannelRuntimeCommand::SetModel(model))
            }
        }
        "/new" => Some(ChannelRuntimeCommand::NewSession),
        _ => None,
    }
}

pub(crate) fn resolve_provider_alias(name: &str) -> Option<String> {
    let candidate = name.trim();
    if candidate.is_empty() {
        return None;
    }
    let providers_list = providers::list_providers();
    for provider in providers_list {
        if provider.name.eq_ignore_ascii_case(candidate)
            || provider
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(candidate))
        {
            return Some(provider.name.to_string());
        }
    }

    None
}
