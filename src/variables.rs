use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::IsTerminal;

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

    /// Names the type of the value held under `key`, in the same words a
    /// declared `type` uses. Doubles as the test for whether a name has a
    /// value at all.
    ///
    /// `None` when the variables are not a table of names, because then no
    /// name has a value. `with_defaults` is where such a file is rejected.
    pub fn type_name(&self, key: &str) -> Option<&'static str> {
        match self {
            Variables::Json(serde_json::Value::Object(map)) => {
                map.get(key).map(|value| match value {
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::Bool(_) => "boolean",
                    serde_json::Value::Null => "null",
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::Object(_) => "table",
                })
            }
            Variables::Toml(toml::Value::Table(table)) => table.get(key).map(toml_type_name),
            Variables::Yaml(serde_yaml::Value::Mapping(map)) => map
                .get(serde_yaml::Value::String(key.to_string()))
                .map(|value| match value {
                    serde_yaml::Value::String(_) => "string",
                    serde_yaml::Value::Number(_) => "number",
                    serde_yaml::Value::Bool(_) => "boolean",
                    serde_yaml::Value::Null => "null",
                    serde_yaml::Value::Sequence(_) => "array",
                    serde_yaml::Value::Mapping(_) => "table",
                    serde_yaml::Value::Tagged(_) => "tagged value",
                }),
            _ => None,
        }
    }

    /// Adds each default to `base`, skipping any name `base` already has.
    ///
    /// `base` is what the command line supplied: the `--variables` file with
    /// any `--var-json` values already merged in, or `None` if neither was
    /// given. Those names were asked for explicitly, so they must win, and
    /// skipping them is how a declared default loses to them.
    ///
    /// `defaults` are the variables a script declares in its own
    /// `config.toml`. `base` keeps its own format, and each default is
    /// converted into that format on the way in.
    pub fn with_defaults(
        base: Option<Variables>,
        defaults: &[(String, toml::Value)],
    ) -> Result<Option<Variables>> {
        if defaults.is_empty() {
            return Ok(base);
        }

        // No file: the declarations are TOML already, so they become the base
        // in their own model and nothing is converted at all.
        let Some(mut base) = base else {
            let mut table = toml::map::Map::new();
            for (key, value) in defaults {
                table.insert(key.clone(), value.clone());
            }
            return Ok(Some(Variables::Toml(toml::Value::Table(table))));
        };

        match &mut base {
            Variables::Json(serde_json::Value::Object(map)) => {
                for (key, value) in defaults {
                    if map.contains_key(key) {
                        continue;
                    }
                    let value = serde_json::Value::deserialize(value.clone())
                        .map_err(|e| default_not_representable(key, "JSON", e))?;
                    map.insert(key.clone(), value);
                }
            }
            Variables::Toml(toml::Value::Table(table)) => {
                for (key, value) in defaults {
                    if table.contains_key(key) {
                        continue;
                    }
                    table.insert(key.clone(), value.clone());
                }
            }
            Variables::Yaml(serde_yaml::Value::Mapping(map)) => {
                for (key, value) in defaults {
                    let key = serde_yaml::Value::String(key.clone());
                    if map.contains_key(&key) {
                        continue;
                    }
                    let value = serde_yaml::Value::deserialize(value.clone()).map_err(|e| {
                        default_not_representable(key.as_str().unwrap_or_default(), "YAML", e)
                    })?;
                    map.insert(key, value);
                }
            }
            _ => return Err(top_level_not_a_table()),
        }

        Ok(Some(base))
    }
}

/// A declared value the variables file's data model cannot hold. Separate from
/// the override message so it points at `config.toml` rather than a flag.
fn default_not_representable(
    key: &str,
    format: &str,
    err: impl std::fmt::Display,
) -> anyhow::Error {
    anyhow!(
        "variable '{}' declared in config.toml cannot be merged into a {} variables file: {}",
        key,
        format,
        err
    )
}

fn top_level_not_a_table() -> anyhow::Error {
    anyhow!("--var-json requires the variables file's top level to be a table of keys")
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

/// The type a declared variable's value carries, so a prompted number reaches
/// SQL as a number rather than a quoted literal — the same distinction
/// `--var-json` makes between `42` and `"42"`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VariableType {
    #[default]
    String,
    Number,
    Boolean,
}

impl VariableType {
    fn name(&self) -> &'static str {
        match self {
            VariableType::String => "string",
            VariableType::Number => "number",
            VariableType::Boolean => "boolean",
        }
    }
}

/// Names a TOML value's type in the same words a declared `type` uses.
fn toml_type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) | toml::Value::Float(_) => "number",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
        toml::Value::Datetime(_) => "datetime",
    }
}

/// How a variable declared in a script's `config.toml` gets its value.
///
/// Mirrors [`crate::secrets::SecretSource`]: a `source` key picks the variant,
/// so further sources can be added later without changing what is already
/// written in existing files.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum VariableDefinition {
    /// A value written straight into `config.toml`, in TOML's own types.
    Value { value: toml::Value },
    /// Asked for when the script runs, unless a value was already supplied on
    /// the command line.
    Prompt {
        #[serde(rename = "type", default)]
        value_type: VariableType,
        /// Offered as the pre-filled answer, and used as-is under
        /// [`PromptMode::UseDefaults`].
        #[serde(default)]
        default: Option<toml::Value>,
        /// Shown instead of the bare name when asking.
        #[serde(default)]
        description: Option<String>,
    },
}

impl VariableDefinition {
    /// Catches a default that contradicts its declared `type` when the config
    /// is read, rather than when a prompt eventually tries to use it.
    pub fn validate(&self, name: &str) -> Result<()> {
        if let VariableDefinition::Prompt {
            value_type,
            default: Some(default),
            ..
        } = self
        {
            if toml_type_name(default) != value_type.name() {
                return Err(anyhow!(
                    "variable '{}': default {} is not a {}",
                    name,
                    default,
                    value_type.name()
                ));
            }
        }
        Ok(())
    }
}

/// Whether a declared variable still missing a value may be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMode {
    /// Ask for anything still missing.
    Ask,
    /// Do not ask; take the declared default, and fail where there is none.
    UseDefaults,
    /// Do not ask and do not fall back to a default: missing is an error.
    Never,
}

impl PromptMode {
    /// The one place that decides whether a command may ask a question.
    ///
    /// `--use-defaults` wins outright. Otherwise anything unattended refuses
    /// to ask: `unattended` for a command with a `--yes` to pass, and in
    /// either case a stdin with no terminal behind it. `script build` has no
    /// `--yes`, so it passes `false` and is decided by the terminal alone.
    ///
    /// stdin is the stream to test, not stdout: prompts are written to stderr
    /// but read from stdin, so `spawn script build x > out.sql` can still ask.
    pub fn resolve(use_defaults: bool, unattended: bool) -> Self {
        if use_defaults {
            PromptMode::UseDefaults
        } else if unattended || !std::io::stdin().is_terminal() {
            PromptMode::Never
        } else {
            PromptMode::Ask
        }
    }
}

/// Resolves the variables a script declares in its `config.toml`, skipping any
/// the command line already supplied.
///
/// Only names listed there count. Writing `{{ variables.x }}` in the SQL
/// declares nothing: an undeclared name is never prompted for, and has to come
/// from `--variables` or `--var-json` or the render fails as undefined.
///
/// Returned in declaration order, which is the order prompts are shown in, and
/// applied by [`Variables::with_defaults`].
pub fn resolve_declared(
    declared: &BTreeMap<String, VariableDefinition>,
    supplied: Option<&Variables>,
    mode: PromptMode,
) -> Result<Vec<(String, toml::Value)>> {
    let mut resolved = Vec::new();

    for (name, definition) in declared {
        definition.validate(name)?;

        // Supplied by --variables or --var-json: treat it as answered, once it
        // matches the declared type. This is what keeps `--var-json
        // 'email="..."'` from prompting anyway.
        if let Some(supplied) = supplied.and_then(|v| v.type_name(name)) {
            if let VariableDefinition::Prompt { value_type, .. } = definition {
                if supplied != value_type.name() {
                    return Err(wrong_type(name, *value_type, supplied));
                }
            }
            continue;
        }

        let value = match definition {
            VariableDefinition::Value { value } => value.clone(),
            VariableDefinition::Prompt {
                value_type,
                default,
                description,
            } => match mode {
                PromptMode::Ask => {
                    ask_for(name, *value_type, default.as_ref(), description.as_deref())?
                }
                PromptMode::UseDefaults => default
                    .clone()
                    .ok_or_else(|| no_default_to_use(name, *value_type))?,
                PromptMode::Never => return Err(cannot_ask_for(name, *value_type)),
            },
        };

        resolved.push((name.clone(), value));
    }

    Ok(resolved)
}

/// Shown when prompting is off — `--yes`, or no terminal to ask at — and the
/// value was not supplied. Names both ways out so a failing CI job says what
/// to add.
fn cannot_ask_for(name: &str, value_type: VariableType) -> anyhow::Error {
    anyhow!(
        "variable '{}' needs a {} value and cannot be asked for here: pass --var-json '{}', \
         or --use-defaults to take the declared default",
        name,
        value_type.name(),
        example_for(name, value_type)
    )
}

/// Shown when a supplied value contradicts the declared `type`. Spells out the
/// quoting, since the usual cause is `--var-json 'x=42'` where `x='"42"'` was
/// meant.
fn wrong_type(name: &str, value_type: VariableType, supplied: &str) -> anyhow::Error {
    anyhow!(
        "variable '{}' is declared as a {}, but the supplied value is of type {}: \
         pass --var-json '{}'",
        name,
        value_type.name(),
        supplied,
        example_for(name, value_type)
    )
}

fn no_default_to_use(name: &str, value_type: VariableType) -> anyhow::Error {
    anyhow!(
        "variable '{}' declares no default for --use-defaults to take: pass --var-json '{}'",
        name,
        example_for(name, value_type)
    )
}

/// A copy-pasteable `--var-json` argument, correctly quoted for the type —
/// the quoting is the easiest thing to get wrong.
fn example_for(name: &str, value_type: VariableType) -> String {
    match value_type {
        VariableType::String => format!("{}=\"...\"", name),
        VariableType::Number => format!("{}=0", name),
        VariableType::Boolean => format!("{}=true", name),
    }
}

/// Asks for one value on the terminal, parsed into its declared type.
fn ask_for(
    name: &str,
    value_type: VariableType,
    default: Option<&toml::Value>,
    description: Option<&str>,
) -> Result<toml::Value> {
    let label = match description {
        Some(description) => format!("{} ({})", description, name),
        None => name.to_string(),
    };

    match value_type {
        VariableType::Boolean => {
            // wait_for_newline for the same reason the run confirmation does:
            // an unconsumed Enter would be read as an empty answer by whatever
            // variable is asked for next.
            let mut confirm = Confirm::new().with_prompt(label).wait_for_newline(true);
            if let Some(toml::Value::Boolean(default)) = default {
                confirm = confirm.default(*default);
            }
            Ok(toml::Value::Boolean(confirm.interact()?))
        }
        VariableType::Number => {
            let mut input = Input::<String>::new().with_prompt(label);
            if let Some(default) = default {
                input = input.default(plain_text(default));
            }
            let text = input
                .validate_with(|text: &String| match parse_number(text) {
                    Some(_) => Ok(()),
                    None => Err("expected a number"),
                })
                .interact_text()?;
            // Already validated above, so the parse cannot fail here.
            parse_number(&text).ok_or_else(|| anyhow!("'{}' is not a number", text))
        }
        VariableType::String => {
            let mut input = Input::<String>::new().with_prompt(label).allow_empty(true);
            if let Some(default) = default {
                input = input.default(plain_text(default));
            }
            Ok(toml::Value::String(input.interact_text()?))
        }
    }
}

/// An integer where the text is one, so `42` does not become `42.0` and reach
/// SQL as a float.
fn parse_number(text: &str) -> Option<toml::Value> {
    let text = text.trim();
    if let Ok(integer) = text.parse::<i64>() {
        return Some(toml::Value::Integer(integer));
    }
    text.parse::<f64>().ok().map(toml::Value::Float)
}

/// A scalar as the user would type it — `toml::Value`'s own Display quotes
/// strings, which is wrong for a pre-filled prompt answer.
fn plain_text(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
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

    #[test]
    fn a_value_that_is_not_json_is_rejected_with_the_quoting_hint() {
        let err = parse_var_override("tenant=acme").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid --var-json 'tenant=acme'"),
            "got: {}",
            msg
        );
        // The likeliest mistake is a bare string, so the error has to say so.
        assert!(msg.contains(r#"tenant='"text"'"#), "got: {}", msg);
    }

    #[test]
    fn an_object_override_replaces_the_key_rather_than_merging_into_it() {
        let base = Variables::from_str("toml", "[limits]\nmax = 5\nmin = 1\n").unwrap();
        let result =
            Variables::with_overrides(Some(base), &overrides(&[("limits", r#"{"max":10}"#)]))
                .unwrap()
                .unwrap();
        // `min` is gone: the whole table was swapped out, not merged into.
        assert_eq!(
            rendered(&result),
            serde_json::json!({"limits": {"max": 10}})
        );
    }

    /// The point of transcoding rather than round-tripping through JSON: a
    /// value only TOML can hold keeps its type when a neighbour is overridden.
    #[test]
    fn a_toml_only_value_survives_a_merge_untouched() {
        let base = Variables::from_str("toml", "created = 2026-11-25T00:00:00Z\nn = 1\n").unwrap();
        let result = Variables::with_overrides(Some(base), &overrides(&[("n", "2")]))
            .unwrap()
            .unwrap();

        // Asserted against the TOML model directly: serializing to JSON would
        // show the toml crate's private datetime marker instead of the type.
        match &result {
            Variables::Toml(toml::Value::Table(table)) => {
                assert!(
                    matches!(table.get("created"), Some(toml::Value::Datetime(_))),
                    "datetime did not survive: {:?}",
                    table.get("created")
                );
                assert_eq!(table.get("n"), Some(&toml::Value::Integer(2)));
            }
            other => panic!("expected a TOML table, got {:?}", other),
        }
    }

    #[test]
    fn null_is_rejected_against_a_toml_file_that_cannot_hold_it() {
        let base = Variables::from_str("toml", "archived = true").unwrap();
        let err =
            Variables::with_overrides(Some(base), &overrides(&[("archived", "null")])).unwrap_err();
        assert!(
            err.to_string().contains("TOML has no null value"),
            "got: {}",
            err
        );
    }

    #[test]
    fn an_integer_past_tomls_range_is_rejected_against_a_toml_file() {
        let base = Variables::from_str("toml", "n = 1").unwrap();
        let err =
            Variables::with_overrides(Some(base), &overrides(&[("n", "18446744073709551615")]))
                .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'n'") && msg.contains("TOML"), "got: {}", msg);
    }

    /// Both limits above are TOML's own, not the flag's — the same overrides
    /// merge cleanly into YAML.
    #[test]
    fn yaml_holds_what_toml_rejects() {
        let base = Variables::from_str(
            "yaml",
            "archived: true
n: 1
",
        )
        .unwrap();
        let result = Variables::with_overrides(
            Some(base),
            &overrides(&[("archived", "null"), ("n", "18446744073709551615")]),
        )
        .unwrap()
        .unwrap();
        assert!(matches!(result, Variables::Yaml(_)));
        assert_eq!(
            rendered(&result),
            serde_json::json!({"archived": null, "n": 18446744073709551615u64})
        );
    }

    #[test]
    fn overrides_with_no_file_need_no_conversion_at_all() {
        let result = Variables::with_overrides(None, &overrides(&[("nothing", "null")]))
            .unwrap()
            .unwrap();
        // JSON is the override's own model, so null is fine with no file.
        assert!(matches!(result, Variables::Json(_)));
        assert_eq!(rendered(&result), serde_json::json!({"nothing": null}));
    }

    /// One `source = "prompt"` declaration, written the way a `config.toml`
    /// writes it.
    fn prompted(value_type: &str) -> BTreeMap<String, VariableDefinition> {
        toml::from_str(&format!(
            "[email]\nsource = \"prompt\"\ntype = \"{}\"\n",
            value_type
        ))
        .unwrap()
    }

    /// `Never` so nothing can prompt: a supplied value is the only way past.
    fn resolve(
        declared: &BTreeMap<String, VariableDefinition>,
        supplied: &Variables,
    ) -> Result<Vec<(String, toml::Value)>> {
        resolve_declared(declared, Some(supplied), PromptMode::Never)
    }

    #[test]
    fn a_supplied_value_must_match_the_declared_type() {
        for supplied in ["42", "true", "null", "[1,2]", r#"{"a":1}"#] {
            let vars = Variables::with_overrides(None, &overrides(&[("email", supplied)]))
                .unwrap()
                .unwrap();
            let err = resolve(&prompted("string"), &vars).unwrap_err();
            assert!(
                err.to_string().contains("declared as a string"),
                "{} gave: {}",
                supplied,
                err
            );
        }
    }

    #[test]
    fn a_supplied_value_of_the_declared_type_is_accepted() {
        let vars = Variables::with_overrides(None, &overrides(&[("email", r#""ok""#)]))
            .unwrap()
            .unwrap();
        // Empty because a supplied name is already answered: nothing to add.
        assert!(resolve(&prompted("string"), &vars).unwrap().is_empty());
    }

    #[test]
    fn a_string_supplied_for_a_number_is_caught() {
        // The quiet direction: Postgres coerces '42' to 42, so without this
        // check nothing downstream would have complained.
        let vars = Variables::with_overrides(None, &overrides(&[("email", r#""42""#)]))
            .unwrap()
            .unwrap();
        let err = resolve(&prompted("number"), &vars).unwrap_err();
        assert!(err.to_string().contains("declared as a number"), "{}", err);
    }

    #[test]
    fn the_type_is_enforced_whatever_format_the_variables_file_is() {
        for (format, text) in [
            ("json", r#"{"email": 42}"#),
            ("toml", "email = 42"),
            ("yaml", "email: 42"),
        ] {
            let vars = Variables::from_str(format, text).unwrap();
            let err = resolve(&prompted("string"), &vars).unwrap_err();
            assert!(
                err.to_string().contains("is of type number"),
                "{} gave: {}",
                format,
                err
            );
        }
    }

    #[test]
    fn a_declaration_that_carries_its_own_value_has_no_type_to_enforce() {
        let declared: BTreeMap<String, VariableDefinition> =
            toml::from_str("[email]\nsource = \"value\"\nvalue = \"declared\"\n").unwrap();
        let vars = Variables::with_overrides(None, &overrides(&[("email", "42")]))
            .unwrap()
            .unwrap();
        assert!(resolve(&declared, &vars).unwrap().is_empty());
    }

    #[test]
    fn a_default_that_contradicts_its_type_is_still_rejected() {
        let declared: BTreeMap<String, VariableDefinition> =
            toml::from_str("[email]\nsource = \"prompt\"\ntype = \"number\"\ndefault = \"8080\"\n")
                .unwrap();
        let err = resolve_declared(&declared, None, PromptMode::UseDefaults).unwrap_err();
        assert!(err.to_string().contains("is not a number"), "{}", err);
    }
}
