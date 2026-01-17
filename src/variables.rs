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

impl Variables {
    pub fn from_str(s_type: &str, s: &str) -> Result<Self> {
        match s_type {
            "json" => {
                let value: serde_json::Value =
                    serde_json::from_str(s).map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
                Ok(Variables::Json(value))
            }
            "toml" => {
                let value: toml::Value =
                    toml::from_str(s).map_err(|e| anyhow::anyhow!("Invalid TOML: {}", e))?;
                Ok(Variables::Toml(value))
            }
            "yaml" | "yml" => {
                let value: serde_yaml::Value =
                    serde_yaml::from_str(s).map_err(|e| anyhow::anyhow!("Invalid YAML: {}", e))?;
                Ok(Variables::Yaml(value))
            }
            _ => Err(anyhow::anyhow!(
                "Unsupported file format (expected .json, .toml, or .yaml)"
            )),
        }
    }
}
