#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

/// A fixture definition loaded from YAML
#[derive(Debug, Deserialize)]
pub struct FixtureDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub files: HashMap<String, String>,
    #[serde(default)]
    pub expected: FixtureExpectations,
}

/// Expected results for a fixture
#[derive(Debug, Default, Deserialize)]
pub struct FixtureExpectations {
    #[serde(default)]
    pub symbols: Vec<ExpectedSymbol>,
    #[serde(default)]
    pub references: HashMap<String, Vec<ExpectedReference>>,
    #[serde(default)]
    pub dependencies: HashMap<String, Vec<String>>,
}

/// Expected symbol in a fixture
#[derive(Debug, Deserialize)]
pub struct ExpectedSymbol {
    pub name: String,
    pub kind: String,
    pub file: String,
    #[serde(default)]
    pub visibility: Option<String>,
}

/// Expected reference in a fixture
#[derive(Debug, Deserialize)]
pub struct ExpectedReference {
    pub file: String,
    pub count: usize,
}

impl FixtureDefinition {
    /// Load a fixture by name from the tests/fixtures directory
    pub fn load(fixture_name: &str) -> Result<Self> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("{}.yaml", fixture_name));
        let content = std::fs::read_to_string(&path)?;
        Ok(serde_yaml::from_str(&content)?)
    }

    /// Load a fixture from a specific path
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_yaml::from_str(&content)?)
    }
}
