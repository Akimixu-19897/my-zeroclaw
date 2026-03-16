use super::*;

pub mod bitable;
pub mod calendar;
pub mod common;
pub mod doc_create;
pub mod doc_fetch;
pub mod doc_update;
pub mod drive_file;
pub mod im_message;
pub mod im_read;
pub mod im_resource;
pub mod search;
pub mod sheets;
pub mod task;
pub mod wiki_space;

pub use bitable::FeishuBitableTool;
pub use calendar::FeishuCalendarTool;
pub use doc_create::FeishuDocCreateTool;
pub use doc_fetch::FeishuDocFetchTool;
pub use doc_update::FeishuDocUpdateTool;
pub use drive_file::FeishuDriveFileTool;
pub use im_message::FeishuImMessageTool;
pub use im_read::FeishuImReadTool;
pub use im_resource::FeishuImResourceTool;
pub use search::FeishuSearchTool;
pub use sheets::FeishuSheetsTool;
pub use task::FeishuTaskTool;
pub use wiki_space::FeishuWikiSpaceTool;

pub(super) fn append_feishu_tools(
    tool_arcs: &mut Vec<Arc<dyn Tool>>,
    config: &Arc<Config>,
    workspace_dir: &std::path::Path,
) {
    tool_arcs.push(Arc::new(FeishuCalendarTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuDocCreateTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuDocFetchTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuDocUpdateTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuDriveFileTool::new(
        config.clone(),
        workspace_dir.to_path_buf(),
    )));
    tool_arcs.push(Arc::new(FeishuBitableTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuImReadTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuImMessageTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuImResourceTool::new(
        config.clone(),
        workspace_dir.to_path_buf(),
    )));
    tool_arcs.push(Arc::new(FeishuSearchTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuSheetsTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuTaskTool::new(config.clone())));
    tool_arcs.push(Arc::new(FeishuWikiSpaceTool::new(config.clone())));
}
