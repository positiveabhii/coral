//! Focused helpers for the Coral MCP surface.

mod discovery;
mod errors;
mod resources;
mod tools;

pub(crate) use discovery::{
    ColumnSummary, Pagination, RelationSummary, compile_metadata_regex, page_items, paged_value,
    parse_pagination, parse_pagination_with_limits,
};
pub(crate) use errors::{
    internal_status, status_to_error_data, tool_error_from_status, tool_error_result,
};
pub(crate) use resources::{
    guide_resource, guide_resource_content, initial_instructions, list_relations_value,
    relations_resource, relations_resource_content,
};
pub(crate) use tools::{
    build_tool_result, describe_relation_arguments, describe_relation_tool, feedback_tool,
    list_columns_arguments, list_columns_tool, list_relations_arguments, list_relations_tool,
    required_string_argument, search_relations_arguments, search_relations_tool, sql_tool,
};
