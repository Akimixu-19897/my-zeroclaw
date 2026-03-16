#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeishuOwnerPolicyDisposition {
    SafeDefault,
    Restricted,
    ReviewRecommended,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FeishuOwnerPolicyReport {
    pub disposition: FeishuOwnerPolicyDisposition,
    pub summary: String,
}

pub fn evaluate_feishu_owner_policy(
    allowed_users: &[String],
    mention_only: bool,
) -> FeishuOwnerPolicyReport {
    let wildcard_allowed = allowed_users.iter().any(|user| user == "*");

    if allowed_users.is_empty() {
        return FeishuOwnerPolicyReport {
            disposition: FeishuOwnerPolicyDisposition::ReviewRecommended,
            summary: "allowed_users is empty; channel will ignore all senders until an allowlist is configured".to_string(),
        };
    }

    if wildcard_allowed && !mention_only {
        return FeishuOwnerPolicyReport {
            disposition: FeishuOwnerPolicyDisposition::ReviewRecommended,
            summary: "wildcard allowed_users with mention_only disabled can expose group chats broadly; prefer explicit user allowlists or mention-only groups".to_string(),
        };
    }

    if wildcard_allowed && mention_only {
        return FeishuOwnerPolicyReport {
            disposition: FeishuOwnerPolicyDisposition::SafeDefault,
            summary: "wildcard allowlist is mitigated by mention_only group gating".to_string(),
        };
    }

    if mention_only {
        return FeishuOwnerPolicyReport {
            disposition: FeishuOwnerPolicyDisposition::Restricted,
            summary: "explicit allowlist plus mention_only provides restrictive DM/group defaults"
                .to_string(),
        };
    }

    FeishuOwnerPolicyReport {
        disposition: FeishuOwnerPolicyDisposition::Restricted,
        summary:
            "explicit allowlist configured; review whether group mention gating is also desired"
                .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_requires_review() {
        let report = evaluate_feishu_owner_policy(&[], false);
        assert_eq!(
            report.disposition,
            FeishuOwnerPolicyDisposition::ReviewRecommended
        );
    }

    #[test]
    fn wildcard_without_mentions_is_review_recommended() {
        let report = evaluate_feishu_owner_policy(&["*".to_string()], false);
        assert_eq!(
            report.disposition,
            FeishuOwnerPolicyDisposition::ReviewRecommended
        );
    }

    #[test]
    fn explicit_allowlist_and_mentions_is_restricted() {
        let report = evaluate_feishu_owner_policy(&["ou_123".to_string()], true);
        assert_eq!(report.disposition, FeishuOwnerPolicyDisposition::Restricted);
    }
}
