// File System MCP provider - modular structure
// Handles file operations and HTML to Markdown conversion

pub mod core;
pub mod file_ops;
pub mod text_editing;
pub mod directory;
pub mod html_converter;
pub mod functions;

// Re-export main functionality
pub use functions::get_all_functions;
pub use core::{execute_text_editor, execute_line_replace, execute_list_files, execute_view_many, execute_html2md};