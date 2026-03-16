use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn feishu_parity_matrix_tracks_expected_source_modules() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected_paths = [
        "src/channels/lark/inbound.rs",
        "src/channels/lark/media.rs",
        "src/channels/lark/outbound.rs",
        "src/channels/lark/cards.rs",
        "src/channels/lark/runtime.rs",
        "src/tools/feishu/im_read.rs",
        "src/tools/feishu/im_message.rs",
        "src/tools/feishu/im_resource.rs",
        "src/tools/feishu/doc_create.rs",
        "src/tools/feishu/doc_fetch.rs",
        "src/tools/feishu/doc_update.rs",
        "src/tools/feishu/drive_file.rs",
        "src/tools/feishu/wiki_space.rs",
        "src/tools/feishu/bitable.rs",
        "src/tools/feishu/sheets.rs",
        "src/tools/feishu/calendar.rs",
        "src/tools/feishu/task.rs",
        "src/tools/feishu/search.rs",
        "src/auth/feishu_oauth.rs",
        "src/security/feishu_scopes.rs",
        "src/security/feishu_owner_policy.rs",
        "src/doctor/feishu.rs",
    ];

    let missing: Vec<PathBuf> = expected_paths
        .iter()
        .map(|relative| root.join(relative))
        .filter(|path| !path.exists())
        .collect();

    assert!(
        missing.is_empty(),
        "missing feishu parity modules: {missing:?}"
    );
}

#[test]
fn feishu_parity_matrix_tracks_intentional_differences_section_in_plan() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plan =
        fs::read_to_string(root.join("docs/plans/2026-03-13-feishu-plugin-parity-plan.zh-CN.md"))
            .expect("plan");

    assert!(
        plan.contains("官方行为差异") || plan.contains("有意差异"),
        "plan should track remaining intentional/official behavior differences"
    );
}

#[test]
fn feishu_parity_matrix_tracks_phase_4_4_to_5_2_plan_sections() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plan =
        fs::read_to_string(root.join("docs/plans/2026-03-13-feishu-plugin-parity-plan.zh-CN.md"))
            .expect("plan");

    for heading in [
        "### Phase 4.4: Onboarding Migration Parity",
        "### Phase 5.1: Doctor / Diagnose Commands",
        "### Phase 5.2: Parity Test Matrix",
    ] {
        assert!(
            plan.contains(heading),
            "plan should retain parity milestone heading: {heading}"
        );
    }
}
