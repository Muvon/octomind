//! Language support module for the indexer
//! Provides a common interface for language-specific parsing and symbol extraction

use tree_sitter::Node;

// Import all language modules
mod javascript;
mod rust;
mod python;
mod go;
mod cpp;
mod php;
mod bash;
mod ruby;
mod typescript;
mod json;

// Re-export language modules
pub use javascript::JavaScript;
pub use rust::Rust;
pub use python::Python;
pub use go::Go;
pub use cpp::Cpp;
pub use php::Php;
pub use bash::Bash;
pub use ruby::Ruby;
pub use typescript::TypeScript;
pub use json::Json;

/// Common trait for all language parsers
pub trait Language {
    /// Name of the language
    fn name(&self) -> &'static str;

    /// Get tree-sitter language for parsing
    fn get_ts_language(&self) -> tree_sitter::Language;

    /// Returns node kinds considered meaningful for this language
    fn get_meaningful_kinds(&self) -> Vec<&'static str>;

    /// Extract symbols from a node
    fn extract_symbols(&self, node: Node, contents: &str) -> Vec<String>;

    /// Extract identifiers from a node (helper method)
    fn extract_identifiers(&self, node: Node, contents: &str, symbols: &mut Vec<String>);
}

/// Gets a language implementation by its name
pub fn get_language(name: &str) -> Option<Box<dyn Language>> {
    match name {
        "rust" => Some(Box::new(Rust {})),
        "javascript" => Some(Box::new(JavaScript {})),
        "typescript" => Some(Box::new(TypeScript {})),
        "python" => Some(Box::new(Python {})),
        "go" => Some(Box::new(Go {})),
        "cpp" => Some(Box::new(Cpp {})),
        "php" => Some(Box::new(Php {})),
        "bash" => Some(Box::new(Bash {})),
        "ruby" => Some(Box::new(Ruby {})),
        "json" => Some(Box::new(Json {})),
        _ => None,
    }
}
