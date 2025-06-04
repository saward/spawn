use std::fs;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use serde::Serialize;

#[derive(Clone, Debug)]
pub enum Variables {
    Json(serde_json::Value),
    Toml(toml::Value),
    Yaml(serde_yaml::Value),
}

impl Serialize for Variables {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Variables::Json(v) => v.serialize(serializer),
            Variables::Toml(v) => v.serialize(serializer),
            Variables::Yaml(v) => v.serialize(serializer),
        }
    }
}

impl Default for Variables {
    fn default() -> Self {
        Self::Json(serde_json::Value::default())
    }
}

impl FromStr for Variables {
    type Err = String;

    fn from_str(path_str: &str) -> Result<Self, Self::Err> {
        let path = Path::new(path_str);
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;

        match path.extension().and_then(|s| s.to_str()) {
            Some("json") => {
                let value: serde_json::Value =
                    serde_json::from_str(&content).map_err(|e| format!("Invalid JSON: {}", e))?;
                Ok(Variables::Json(value))
            }
            Some("toml") => {
                let value: toml::Value =
                    toml::from_str(&content).map_err(|e| format!("Invalid TOML: {}", e))?;
                Ok(Variables::Toml(value))
            }
            Some("yaml") | Some("yml") => {
                let value: serde_yaml::Value =
                    serde_yaml::from_str(&content).map_err(|e| format!("Invalid YAML: {}", e))?;
                Ok(Variables::Yaml(value))
            }
            _ => Err("Unsupported file format (expected .json, .toml, or .yaml)".into()),
        }
    }
}
