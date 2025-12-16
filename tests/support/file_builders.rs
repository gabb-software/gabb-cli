#![allow(dead_code)]

use std::path::PathBuf;

use super::workspace::FileContent;

/// Trait for collecting files back into parent builder
pub trait FileCollector {
    fn collect_file(self, path: PathBuf, content: FileContent) -> Self;
}

/// Import definition for TypeScript files
pub struct Import {
    pub from: String,
    pub symbols: Vec<ImportSymbol>,
}

pub struct ImportSymbol {
    pub name: String,
    pub alias: Option<String>,
}

/// Builder for TypeScript file content
pub struct TsFileBuilder<P> {
    parent: P,
    path: PathBuf,
    imports: Vec<Import>,
    exports: Vec<String>,
    body: Vec<String>,
}

impl<P> TsFileBuilder<P> {
    pub fn new(parent: P, path: PathBuf) -> Self {
        Self {
            parent,
            path,
            imports: Vec::new(),
            exports: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Add an import statement
    pub fn importing(mut self, from: impl Into<String>, symbols: &[&str]) -> Self {
        self.imports.push(Import {
            from: from.into(),
            symbols: symbols
                .iter()
                .map(|s| ImportSymbol {
                    name: s.to_string(),
                    alias: None,
                })
                .collect(),
        });
        self
    }

    /// Add an aliased import
    pub fn importing_as(mut self, from: impl Into<String>, name: &str, alias: &str) -> Self {
        self.imports.push(Import {
            from: from.into(),
            symbols: vec![ImportSymbol {
                name: name.to_string(),
                alias: Some(alias.to_string()),
            }],
        });
        self
    }

    /// Export a function
    pub fn with_function(mut self, name: &str, body: &str) -> Self {
        self.exports
            .push(format!("export function {}() {{ {} }}", name, body));
        self
    }

    /// Export an interface
    pub fn with_interface(mut self, name: &str, members: &str) -> Self {
        self.exports
            .push(format!("export interface {} {{ {} }}", name, members));
        self
    }

    /// Export a class
    pub fn with_class(
        mut self,
        name: &str,
        extends: Option<&str>,
        implements: Option<&str>,
        body: &str,
    ) -> Self {
        let mut decl = format!("export class {}", name);
        if let Some(ext) = extends {
            decl.push_str(&format!(" extends {}", ext));
        }
        if let Some(imp) = implements {
            decl.push_str(&format!(" implements {}", imp));
        }
        decl.push_str(&format!(" {{ {} }}", body));
        self.exports.push(decl);
        self
    }

    /// Add arbitrary body content
    pub fn with_body(mut self, content: &str) -> Self {
        self.body.push(content.to_string());
        self
    }

    /// Finish and return to parent builder
    pub fn done(self) -> P
    where
        P: FileCollector,
    {
        let content = self.render();
        self.parent
            .collect_file(self.path, FileContent::Inline(content))
    }

    fn render(&self) -> String {
        let mut parts = Vec::new();

        // Imports
        for import in &self.imports {
            let symbols: Vec<String> = import
                .symbols
                .iter()
                .map(|s| {
                    if let Some(alias) = &s.alias {
                        format!("{} as {}", s.name, alias)
                    } else {
                        s.name.clone()
                    }
                })
                .collect();
            parts.push(format!(
                "import {{ {} }} from '{}';",
                symbols.join(", "),
                import.from
            ));
        }

        // Exports and body
        parts.extend(self.exports.iter().cloned());
        parts.extend(self.body.iter().cloned());

        parts.join("\n")
    }
}

/// Builder for Rust file content
pub struct RsFileBuilder<P> {
    parent: P,
    path: PathBuf,
    uses: Vec<String>,
    mods: Vec<String>,
    items: Vec<String>,
}

impl<P> RsFileBuilder<P> {
    pub fn new(parent: P, path: PathBuf) -> Self {
        Self {
            parent,
            path,
            uses: Vec::new(),
            mods: Vec::new(),
            items: Vec::new(),
        }
    }

    /// Add a use statement
    pub fn using(mut self, path: impl Into<String>) -> Self {
        self.uses.push(format!("use {};", path.into()));
        self
    }

    /// Add a mod declaration
    pub fn with_mod(mut self, name: &str) -> Self {
        self.mods.push(format!("mod {};", name));
        self
    }

    /// Add a public function
    pub fn with_pub_fn(mut self, name: &str, body: &str) -> Self {
        self.items.push(format!("pub fn {}() {{ {} }}", name, body));
        self
    }

    /// Add a function (not public)
    pub fn with_fn(mut self, name: &str, body: &str) -> Self {
        self.items.push(format!("fn {}() {{ {} }}", name, body));
        self
    }

    /// Add a struct
    pub fn with_struct(mut self, name: &str, fields: &str) -> Self {
        self.items
            .push(format!("pub struct {} {{ {} }}", name, fields));
        self
    }

    /// Add a trait
    pub fn with_trait(mut self, name: &str, methods: &str) -> Self {
        self.items
            .push(format!("pub trait {} {{ {} }}", name, methods));
        self
    }

    /// Add an impl block
    pub fn with_impl(mut self, trait_name: Option<&str>, for_type: &str, body: &str) -> Self {
        if let Some(tr) = trait_name {
            self.items
                .push(format!("impl {} for {} {{ {} }}", tr, for_type, body));
        } else {
            self.items.push(format!("impl {} {{ {} }}", for_type, body));
        }
        self
    }

    /// Add arbitrary content
    pub fn with_body(mut self, content: &str) -> Self {
        self.items.push(content.to_string());
        self
    }

    /// Finish and return to parent builder
    pub fn done(self) -> P
    where
        P: FileCollector,
    {
        let content = self.render();
        self.parent
            .collect_file(self.path, FileContent::Inline(content))
    }

    fn render(&self) -> String {
        let mut parts = Vec::new();
        parts.extend(self.mods.iter().cloned());
        parts.extend(self.uses.iter().cloned());
        if !self.mods.is_empty() || !self.uses.is_empty() {
            parts.push(String::new());
        }
        parts.extend(self.items.iter().cloned());
        parts.join("\n")
    }
}
