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

    /// Layers `--var key=value` overrides on top of `base`; `None` when there
    /// is nothing at all to pass.
    ///
    /// Overrides stay strings — inferring numbers or booleans would silently
    /// change how a dialect escapes them; use a typed `--variables` file for
    /// non-strings. The merge keeps the base's own data model.
    pub fn with_overrides(
        base: Option<Variables>,
        overrides: &[(String, String)],
    ) -> Result<Option<Variables>> {
        if overrides.is_empty() {
            return Ok(base);
        }

        // No file: JSON is the neutral home, string overrides being identical in every model.
        let mut base =
            base.unwrap_or_else(|| Variables::Json(serde_json::Value::Object(Default::default())));

        // Outer match is exhaustive on purpose: a new variant must fail to compile here.
        match &mut base {
            Variables::Json(value) => match value {
                serde_json::Value::Object(map) => {
                    for (key, value) in overrides {
                        map.insert(key.clone(), serde_json::Value::String(value.clone()));
                    }
                }
                _ => return Err(top_level_not_a_table()),
            },
            Variables::Toml(value) => match value {
                toml::Value::Table(table) => {
                    for (key, value) in overrides {
                        table.insert(key.clone(), toml::Value::String(value.clone()));
                    }
                }
                // Unreachable from a parsed file – a TOML document is always a table.
                _ => return Err(top_level_not_a_table()),
            },
            Variables::Yaml(value) => match value {
                serde_yaml::Value::Mapping(map) => {
                    for (key, value) in overrides {
                        map.insert(
                            serde_yaml::Value::String(key.clone()),
                            serde_yaml::Value::String(value.clone()),
                        );
                    }
                }
                _ => return Err(top_level_not_a_table()),
            },
        }

        Ok(Some(base))
    }
}

fn top_level_not_a_table() -> anyhow::Error {
    anyhow::anyhow!("--var requires the variables file's top level to be a table of keys")
}

/// Parses a single `--var key=value` argument, splitting on the first `=`
/// only, so values may contain `=` themselves.
pub fn parse_var_override(arg: &str) -> Result<(String, String)> {
    let (key, value) = arg
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid --var '{}': expected key=value", arg))?;
    if key.is_empty() {
        return Err(anyhow::anyhow!("invalid --var '{}': key is empty", arg));
    }
    // '.' is reserved so nested overrides (--var a.b=c) can be added later.
    if key.contains('.') {
        return Err(anyhow::anyhow!(
            "invalid --var '{}': '.' is reserved in keys for future nested overrides",
            arg
        ));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Parses every `--var` argument, collecting them in the order given.
pub fn parse_var_overrides(args: &[String]) -> Result<Vec<(String, String)>> {
    args.iter().map(|arg| parse_var_override(arg)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
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
        assert_eq!(rendered(&result), serde_json::json!({"tenant": "42"}));
    }

    #[test]
    fn an_override_wins_over_the_same_key_in_a_non_json_file() {
        let base = Variables::from_str("yaml", "tenant: from file\nkeep: me\n").unwrap();
        let result = Variables::with_overrides(Some(base), &overrides(&[("tenant", "42")]))
            .unwrap()
            .unwrap();
        assert_eq!(
            rendered(&result),
            serde_json::json!({"tenant": "42", "keep": "me"})
        );
    }

    #[test]
    fn a_numeric_looking_override_stays_a_string() {
        let result = Variables::with_overrides(
            None,
            &overrides(&[("n", "100"), ("flag", "true"), ("nothing", "null")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            rendered(&result),
            serde_json::json!({"n": "100", "flag": "true", "nothing": "null"})
        );
    }

    #[test]
    fn overriding_a_file_that_is_not_a_table_is_an_error() {
        let base = Variables::from_str("json", "[1, 2, 3]").unwrap();
        let err = Variables::with_overrides(Some(base), &overrides(&[("a", "b")])).unwrap_err();
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
            serde_json::json!({"name": "x", "tenant": "42"})
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
        let err = Variables::with_overrides(Some(base), &overrides(&[("a", "b")])).unwrap_err();
        assert!(err.to_string().contains("table of keys"), "got: {}", err);
    }

    #[test]
    fn a_value_may_contain_further_equals_signs() {
        let (key, value) = parse_var_override("dsn=host=db user=app").unwrap();
        assert_eq!(key, "dsn");
        assert_eq!(value, "host=db user=app");
    }

    #[test]
    fn an_override_without_an_equals_sign_is_rejected() {
        let err = parse_var_override("tenant").unwrap_err();
        assert!(
            err.to_string().contains("expected key=value"),
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
