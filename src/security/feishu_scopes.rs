use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuToolScopeRequirements {
    pub tool_name: &'static str,
    pub scopes: &'static [&'static str],
}

const FEISHU_TOOL_SCOPE_REQUIREMENTS: &[FeishuToolScopeRequirements] = &[
    FeishuToolScopeRequirements {
        tool_name: "feishu_im_read",
        scopes: &["im:message"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_im_message",
        scopes: &["im:message", "im:message:send_as_bot"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_im_resource",
        scopes: &["im:message", "im:resource"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_doc_create",
        scopes: &["docs:document", "drive:drive"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_doc_fetch",
        scopes: &["docs:document:readonly"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_doc_update",
        scopes: &["docs:document"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_drive_file",
        scopes: &["drive:drive"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_wiki_space",
        scopes: &["wiki:wiki", "wiki:wiki:readonly"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_bitable",
        scopes: &["bitable:app"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_sheets",
        scopes: &["sheets:spreadsheet"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_calendar",
        scopes: &["calendar:calendar", "calendar:event"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_task",
        scopes: &["task:task", "task:tasklist"],
    },
    FeishuToolScopeRequirements {
        tool_name: "feishu_search",
        scopes: &["search:doc_wiki"],
    },
];

pub fn feishu_tool_scope_requirements(
    tool_name: &str,
) -> Option<&'static FeishuToolScopeRequirements> {
    FEISHU_TOOL_SCOPE_REQUIREMENTS
        .iter()
        .find(|entry| entry.tool_name == tool_name)
}

pub fn all_feishu_tool_scope_requirements() -> &'static [FeishuToolScopeRequirements] {
    FEISHU_TOOL_SCOPE_REQUIREMENTS
}

pub fn missing_scopes_for_tool<'a, I>(
    tool_name: &str,
    granted_scopes: I,
) -> Option<Vec<&'static str>>
where
    I: IntoIterator<Item = &'a str>,
{
    let required = feishu_tool_scope_requirements(tool_name)?;
    let granted: BTreeSet<String> = granted_scopes
        .into_iter()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    let missing = required
        .scopes
        .iter()
        .copied()
        .filter(|scope| !granted.contains(*scope))
        .collect::<Vec<_>>();

    Some(missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tool_returns_scope_requirements() {
        let requirements = feishu_tool_scope_requirements("feishu_doc_create").unwrap();
        assert_eq!(requirements.scopes, &["docs:document", "drive:drive"]);
    }

    #[test]
    fn missing_scopes_reports_only_missing_entries() {
        let missing = missing_scopes_for_tool("feishu_im_message", ["im:message"]).unwrap();
        assert_eq!(missing, vec!["im:message:send_as_bot"]);
    }

    #[test]
    fn unknown_tool_returns_none() {
        assert!(feishu_tool_scope_requirements("shell").is_none());
        assert!(missing_scopes_for_tool("shell", ["x"]).is_none());
    }
}
