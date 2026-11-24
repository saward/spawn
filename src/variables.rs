use anyhow::Result;
use serde::{Deserialize, Serialize};

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

    /// Layers `--var-json key=<json>` overrides on top of `base`; `None` when
    /// there is nothing at all to pass.
    ///
    /// The merge keeps the base's own data model: a TOML file stays TOML and
    /// is never routed through JSON. Only the override crosses, transcoded
    /// value-to-value into whatever the base is, so a TOML datetime sitting
    /// beside an overridden key survives untouched.
    pub fn with_overrides(
        base: Option<Variables>,
        overrides: &[(String, serde_json::Value)],
    ) -> Result<Option<Variables>> {
        if overrides.is_empty() {
            return Ok(base);
        }

        // No file: JSON is the neutral home, and the override's own model, so
        // nothing is converted at all.
        let mut base =
            base.unwrap_or_else(|| Variables::Json(serde_json::Value::Object(Default::default())));

        // Outer match is exhaustive on purpose: a new variant must fail to compile here.
        match &mut base {
            Variables::Json(value) => match value {
                serde_json::Value::Object(map) => {
                    for (key, value) in overrides {
                        map.insert(key.clone(), value.clone());
                    }
                }
                _ => return Err(top_level_not_a_table()),
            },
            Variables::Toml(value) => match value {
                toml::Value::Table(table) => {
                    for (key, value) in overrides {
                        // TOML has no null of its own, so say that rather than
                        // leaving serde's "invalid type" to explain it.
                        if value.is_null() {
                            return Err(no_null_in_toml(key));
                        }
                        let value = toml::Value::deserialize(value.clone())
                            .map_err(|e| not_representable(key, "TOML", e))?;
                        table.insert(key.clone(), value);
                    }
                }
                // Unreachable from a parsed file – a TOML document is always a table.
                _ => return Err(top_level_not_a_table()),
            },
            Variables::Yaml(value) => match value {
                serde_yaml::Value::Mapping(map) => {
                    for (key, value) in overrides {
                        let value = serde_yaml::Value::deserialize(value.clone())
                            .map_err(|e| not_representable(key, "YAML", e))?;
                        map.insert(serde_yaml::Value::String(key.clone()), value);
                    }
                }
                _ => return Err(top_level_not_a_table()),
            },
        }

        Ok(Some(base))
    }
}

fn top_level_not_a_table() -> anyhow::Error {
    anyhow::anyhow!("--var-json requires the variables file's top level to be a table of keys")
}

fn no_null_in_toml(key: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "--var-json '{}=null' cannot be merged into a TOML variables file: TOML has no null value",
        key
    )
}

/// A JSON value the base file's data model cannot hold — a number outside
/// TOML's i64, say. The limit is the format's, not the override syntax's.
fn not_representable(key: &str, format: &str, err: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!(
        "--var-json '{}' cannot be merged into a {} variables file: {}",
        key,
        format,
        err
    )
}

/// Parses a single `--var-json key=<json>` argument, splitting on the first
/// `=` only, so a JSON value may contain `=` itself.
pub fn parse_var_override(arg: &str) -> Result<(String, serde_json::Value)> {
    let (key, value) = arg
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid --var-json '{}': expected key=<json>", arg))?;
    if key.is_empty() {
        return Err(anyhow::anyhow!(
            "invalid --var-json '{}': key is empty",
            arg
        ));
    }
    // '.' is reserved so nested overrides (--var-json a.b=1) can be added later.
    if key.contains('.') {
        return Err(anyhow::anyhow!(
            "invalid --var-json '{}': '.' is reserved in keys for future nested overrides",
            arg
        ));
    }
    // Parsed here rather than at merge time: a malformed value is a problem
    // with the argument, whatever the variables file happens to be.
    let value: serde_json::Value = serde_json::from_str(value).map_err(|e| {
        anyhow::anyhow!(
            "invalid --var-json '{}': {} (a string needs JSON quotes, e.g. {}='\"text\"')",
            arg,
            e,
            key
        )
    })?;
    Ok((key.to_string(), value))
}

/// Parses every `--var-json` argument, collecting them in the order given.
pub fn parse_var_overrides(args: &[String]) -> Result<Vec<(String, serde_json::Value)>> {
    args.iter().map(|arg| parse_var_override(arg)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Takes the raw `key=<json>` text a user would type, so these read the
    /// way the command line does.
    fn overrides(pairs: &[(&str, &str)]) -> Vec<(String, serde_json::Value)> {
        pairs
            .iter()
            .map(|(k, v)| parse_var_override(&format!("{}={}", k, v)).unwrap())
            .collect()
    }

    fn rendered(vars: &Variables) -> serde_json::Value {
        serde_json::to_value(vars).unwrap()
    }

    #[test]
    fn no_overrides_passes_the_base_through_untouched() {
        let base = Variables::from_str("toml", "name = 'from file'").unwrap();
        let result = Variables::with_overrides(Some(base), &[]).unwrap().unwrap();
        assert_eq!(rendered(&result), serde_json::json!({"name": "from file"}));
    }

    #[test]
    fn nothing_at_all_stays_none() {
        assert!(Variables::with_overrides(None, &[]).unwrap().is_none());
    }

    #[test]
    fn overrides_alone_become_the_variables() {
        let result = Variables::with_overrides(None, &overrides(&[("tenant", "42")]))
            .unwrap()
            .unwrap();
        assert_eq!(rendered(&result), serde_json::json!({"tenant": 42}));
    }

    #[test]
    fn an_override_wins_over_the_same_key_in_a_non_json_file() {
        let base = Variables::from_str("yaml", "tenant: from file\nkeep: me\n").unwrap();
        let result = Variables::with_overrides(Some(base), &overrides(&[("tenant", "42")]))
            .unwrap()
            .unwrap();
        assert_eq!(
            rendered(&result),
            serde_json::json!({"tenant": 42, "keep": "me"})
        );
    }

    #[test]
    fn an_override_carries_its_json_type() {
        let result = Variables::with_overrides(
            None,
            &overrides(&[
                ("n", "100"),
                ("flag", "true"),
                ("nothing", "null"),
                ("text", r#""100""#),
                ("tags", r#"["a","b"]"#),
            ]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            rendered(&result),
            serde_json::json!({
                "n": 100, "flag": true, "nothing": null,
                "text": "100", "tags": ["a", "b"],
            })
        );
    }

    #[test]
    fn overriding_a_file_that_is_not_a_table_is_an_error() {
        let base = Variables::from_str("json", "[1, 2, 3]").unwrap();
        let err =
            Variables::with_overrides(Some(base), &overrides(&[("a", r#""b""#)])).unwrap_err();
        assert!(err.to_string().contains("table of keys"), "got: {}", err);
    }

    #[test]
    fn a_merge_keeps_the_bases_native_format() {
        let base = Variables::from_str("toml", "name = 'x'").unwrap();
        let result = Variables::with_overrides(Some(base), &overrides(&[("tenant", "42")]))
            .unwrap()
            .unwrap();
        assert!(matches!(result, Variables::Toml(_)));
        assert_eq!(
            rendered(&result),
            serde_json::json!({"name": "x", "tenant": 42})
        );

        let base = Variables::from_str("yaml", "name: x\n").unwrap();
        let result = Variables::with_overrides(Some(base), &overrides(&[("tenant", "42")]))
            .unwrap()
            .unwrap();
        assert!(matches!(result, Variables::Yaml(_)));
    }

    #[test]
    fn overriding_a_yaml_file_that_is_not_a_table_is_an_error() {
        let base = Variables::from_str("yaml", "just a string").unwrap();
        let err =
            Variables::with_overrides(Some(base), &overrides(&[("a", r#""b""#)])).unwrap_err();
        assert!(err.to_string().contains("table of keys"), "got: {}", err);
    }

    #[test]
    fn a_value_may_contain_further_equals_signs() {
        let (key, value) = parse_var_override(r#"dsn="host=db user=app""#).unwrap();
        assert_eq!(key, "dsn");
        assert_eq!(value, serde_json::json!("host=db user=app"));
    }

    #[test]
    fn an_override_without_an_equals_sign_is_rejected() {
        let err = parse_var_override("tenant").unwrap_err();
        assert!(
            err.to_string().contains("expected key=<json>"),
            "got: {}",
            err
        );
    }

    #[test]
    fn an_override_with_an_empty_key_is_rejected() {
        let err = parse_var_override("=42").unwrap_err();
        assert!(err.to_string().contains("key is empty"), "got: {}", err);
    }

    #[test]
    fn a_dotted_key_is_rejected_as_reserved() {
        let err = parse_var_override("limits.tenant=42").unwrap_err();
        assert!(err.to_string().contains("reserved"), "got: {}", err);
    }
}
