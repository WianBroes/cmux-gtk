//! JSONC reading and `cmux.json` schema validation, independent of GTK and the running app.
//!
//! `cmux.json` is JSONC: `//` and `/* */` comments plus trailing commas are accepted, but only
//! outside string literals. Validation runs against the upstream schema shipped next to this
//! file. Nothing here writes the file, runs a command, or contacts the running application.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The upstream macOS schema for `cmux.json`; authoritative for keys, types and bounds.
const SCHEMA: &str = include_str!("settings_json/cmux.schema.json");

/// Reject oversized files before parsing; the app's own config reads share this budget.
const MAX_BYTES: u64 = 1024 * 1024;

/// Settings that only mean something on macOS, reported as warnings instead of unknown keys.
///
/// `app.appIcon` and `app.menuBarOnly` drive the Dock and menu bar, `notifications.dockBadge`
/// the Dock tile count, and `computerUse.*` / `mobile.*` the macOS automation bridges. The
/// schema is shared verbatim, so these keys are valid JSON that cmux-gtk cannot act on.
pub const MACOS_ONLY_PREFIXES: &[&str] = &["computerUse.", "mobile."];

/// Exact macOS-only settings; kept beside the prefixes so the two never drift apart.
pub const MACOS_ONLY_KEYS: &[&str] = &["app.appIcon", "app.menuBarOnly", "notifications.dockBadge"];

/// A JSONC syntax failure, reported at its position in the original text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsoncError {
    /// One-based line in the original text, before any comment stripping.
    pub line: usize,
    /// One-based column in the original text, counting UTF-8 characters.
    pub column: usize,
    /// Human-readable reason, without the position prefix.
    pub message: String,
}

impl fmt::Display for JsoncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}: {}", self.line, self.column, self.message)
    }
}

impl std::error::Error for JsoncError {}

/// How much a diagnostic matters: errors fail `cmux config validate`, warnings do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The file cannot be used as written.
    Error,
    /// The file is valid but this key or construct does nothing on Linux.
    Warning,
}

/// One validation finding, addressed by dotted settings path (`sidebar.branchLayout`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// Dotted path of the offending setting, or `""` for the document itself.
    pub path: String,
    /// What is wrong, phrased for someone editing the file by hand.
    pub message: String,
    /// Whether the finding blocks use of the file.
    pub severity: Severity,
}

impl Issue {
    /// Errors only, in report order; used for the process exit code.
    pub fn errors(issues: &[Issue]) -> usize {
        issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .count()
    }

    /// `path: message` line used by the text report, with an empty path for document issues.
    pub fn line(&self) -> String {
        if self.path.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.path, self.message)
        }
    }
}

/// One byte of a JSONC document classified for comment removal.
///
/// String bytes stay `Plain`: the scanner skips string spans without ever looking for comment
/// markers inside them, so `"http://x"` is copied through byte for byte.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Ordinary JSON text, copied through unchanged.
    Plain,
    /// Inside a line comment, dropped through the newline.
    LineComment,
    /// Inside a block comment, dropped through the closing delimiter.
    BlockComment,
}

/// One scanner position in the original text, for error reporting only.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;
    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// Classify every byte of the document so comment removal never inspects string contents.
fn classify(text: &str) -> Result<Vec<Class>, JsoncError> {
    let bytes = text.as_bytes();
    let mut classes = vec![Class::Plain; bytes.len()];
    let mut index = 0;
    let mut string_start = None;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
                string_start = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'"' => {
                in_string = true;
                string_start = Some(index);
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                let end = text[index..]
                    .find('\n')
                    .map(|offset| index + offset)
                    .unwrap_or(bytes.len());
                classes[index..end].fill(Class::LineComment);
                index = end;
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                let mut cursor = index + 2;
                let end = loop {
                    if cursor + 1 >= bytes.len() {
                        let (line, column) = position(text, start);
                        return Err(JsoncError {
                            line,
                            column,
                            message: "unterminated block comment".into(),
                        });
                    }
                    if bytes[cursor] == b'*' && bytes[cursor + 1] == b'/' {
                        break cursor + 2;
                    }
                    cursor += 1;
                };
                classes[start..end].fill(Class::BlockComment);
                index = end;
            }
            _ => index += 1,
        }
    }
    if let Some(start) = string_start {
        // Point at the opening quote: that is what the user has to fix or close.
        let (line, column) = position(text, start);
        return Err(JsoncError {
            line,
            column,
            message: "unterminated string".into(),
        });
    }
    Ok(classes)
}

/// Remove `//` and `/* */` comments, replacing each with a space so positions never shift.
///
/// Newlines inside block comments survive, which keeps `serde_json` line numbers meaningful.
fn strip_comments(text: &str, classes: &[Class]) -> String {
    let mut stripped = String::with_capacity(text.len());
    for (index, character) in text.char_indices() {
        if classes.get(index) == Some(&Class::LineComment)
            || classes.get(index) == Some(&Class::BlockComment)
        {
            stripped.push(if character == '\n' { '\n' } else { ' ' });
        } else {
            stripped.push(character);
        }
    }
    stripped
}

/// Drop commas that are only followed by a closing bracket, the JSONC trailing-comma rule.
fn strip_trailing_commas(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut keep = vec![true; bytes.len()];
    let mut in_string = false;
    let mut escaped = false;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        if byte == b'"' {
            in_string = true;
            continue;
        }
        if byte != b',' {
            continue;
        }
        let mut cursor = index + 1;
        while matches!(bytes.get(cursor), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            cursor += 1;
        }
        if matches!(bytes.get(cursor), Some(b'}' | b']')) {
            keep[index] = false;
        }
    }
    let mut stripped = String::with_capacity(text.len());
    for (index, character) in text.char_indices() {
        if keep[index] {
            stripped.push(character);
        }
    }
    stripped
}

/// Parse JSONC text into a value, reporting syntax errors with original line and column.
pub fn parse_jsonc(text: &str) -> Result<Value, JsoncError> {
    let classes = classify(text)?;
    let stripped = strip_trailing_commas(&strip_comments(text, &classes));
    serde_json::from_str(&stripped).map_err(|error| {
        let (line, column) = syntax_position(text, &classes, &error);
        JsoncError {
            line,
            column,
            message: reason(&error),
        }
    })
}

/// The `serde_json` reason with its own trailing position, which `JsoncError` already carries.
fn reason(error: &serde_json::Error) -> String {
    let message = error.to_string();
    let head = message.rfind(" at line ").unwrap_or(message.len());
    message[..head].trim_end().to_owned()
}

/// Locate a `serde_json` failure in the original text, undoing the blanked-comment offset.
fn syntax_position(text: &str, classes: &[Class], error: &serde_json::Error) -> (usize, usize) {
    // The stripped copy is line-for-line identical to the original, so serde_json's line is
    // already the original line; only the column is shifted by bytes blanked earlier on the row.
    let line = error.line().max(1);
    let row_offset = text
        .split('\n')
        .take(line - 1)
        .map(|part| part.len() + 1)
        .sum::<usize>();
    let stripped_column = error.column().saturating_sub(1);
    let mut blanked = 0;
    for offset in 0..stripped_column {
        let index = row_offset + offset;
        if matches!(
            classes.get(index),
            Some(Class::LineComment) | Some(Class::BlockComment)
        ) {
            blanked += 1;
        }
    }
    let column = stripped_column + blanked + 1;
    (line, column)
}

/// The parsed schema, compiled once; validation never rebuilds or mutates it.
pub fn schema() -> &'static Value {
    static SCHEMA_VALUE: OnceLock<Value> = OnceLock::new();
    SCHEMA_VALUE
        .get_or_init(|| serde_json::from_str(SCHEMA).expect("bundled cmux schema is valid JSON"))
}

/// Read one config file as JSONC, treating an absent file as empty settings.
pub fn read(path: &Path) -> Result<Value, String> {
    let Some(bytes) = read_source(path)? else {
        return Ok(Value::Null);
    };
    let text = String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    parse_jsonc(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Read one settings file's raw bytes under the shared budget, with `None` for an absent file.
///
/// Splitting this out of [`read`] is what lets `read_text` serve the editing commands: they
/// need the exact bytes the user wrote, comments included, to edit them in place.
fn read_source(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match cmux_platform::filesystem::open_regular_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    let mut limited = file.take(MAX_BYTES + 1);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{}: settings exceed 1 MiB", path.display()));
    }
    Ok(Some(bytes))
}

/// Whether a settings path is a macOS-only key the Linux port cannot honor.
pub fn is_macos_only(path: &str) -> bool {
    MACOS_ONLY_KEYS.contains(&path)
        || MACOS_ONLY_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// The default global settings path, matching the project-config rule for the same file.
pub fn global_path() -> Option<std::path::PathBuf> {
    super::project_config::global_path()
}

/// Collect every problem in `value`, errors and warnings alike, in document order.
///
/// A `Null` document is the absent-file case: there is nothing to check, so it is not an error.
pub fn validate(value: &Value) -> Vec<Issue> {
    if value.is_null() {
        return Vec::new();
    }
    let mut context = Context {
        root: schema(),
        issues: Vec::new(),
        depth: 0,
    };
    context.check(value, schema(), "");
    let mut issues = macos_only_issues(value);
    // A macOS-only key already carries an "unsupported on Linux" warning; its schema findings
    // would only repeat it, since an inert key cannot misbehave on this platform.
    issues.extend(
        context
            .issues
            .into_iter()
            .filter(|issue| !is_macos_only(&issue.path)),
    );
    issues
}

/// Report every macOS-only setting present in the document, at its full dotted path.
fn macos_only_issues(value: &Value) -> Vec<Issue> {
    fn walk(value: &Value, path: &str, found: &mut Vec<Issue>) {
        match value {
            Value::Object(object) => {
                for (key, entry) in object {
                    let child = join(path, key);
                    if is_macos_only(&child) {
                        found.push(Issue {
                            path: child.clone(),
                            message: "unsupported on Linux: this macOS setting has no effect"
                                .into(),
                            severity: Severity::Warning,
                        });
                    }
                    walk(entry, &child, found);
                }
            }
            Value::Array(items) => {
                for (index, entry) in items.iter().enumerate() {
                    walk(entry, &format!("{path}[{index}]"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(value, "", &mut found);
    found
}

/// Mutable state threaded through one validation pass.
struct Context {
    /// Document root, used to resolve `#/$defs/...` references.
    ///
    /// Held as `'static` so a resolved `$ref` never borrows `self` and the keyword checks can
    /// still take `&mut self` to append findings.
    root: &'static Value,
    /// Problems found so far, appended in traversal order.
    issues: Vec<Issue>,
    /// Current `$ref`/combinator nesting, bounding malformed-schema recursion.
    depth: usize,
}

/// Maximum nesting followed through `$ref`, `allOf` and combinators.
const MAX_DEPTH: usize = 32;

impl Context {
    /// Record one finding unless it repeats an earlier one at the same path and message.
    fn push(&mut self, path: &str, message: String, severity: Severity) {
        let duplicate = self
            .issues
            .iter()
            .any(|issue| issue.path == path && issue.message == message);
        if !duplicate {
            self.issues.push(Issue {
                path: path.to_owned(),
                message,
                severity,
            });
        }
    }

    /// Resolve `#/$defs/name` and relative `#/...` pointers to a concrete subschema.
    ///
    /// The result borrows the document root, not the referring schema, so a `$ref` never
    /// keeps the caller's subschema alive.
    fn resolve(&self, schema: &Value) -> Option<&'static Value> {
        let reference = schema.get("$ref")?.as_str()?;
        let pointer = reference.strip_prefix("#")?;
        self.root.pointer(pointer)
    }

    /// Return the effective subschema, following a `$ref` when present.
    ///
    /// The result is tied to `schema`, never to `&self`, so the caller can keep appending
    /// findings while reading the resolved subschema.
    fn effective<'a>(&self, schema: &'a Value) -> Option<&'a Value> {
        if schema.is_object() && schema.get("$ref").is_some() {
            return self.resolve(schema);
        }
        Some(schema)
    }

    /// Validate one value against one subschema, reporting every keyword it violates.
    fn check(&mut self, value: &Value, schema: &Value, path: &str) {
        if self.depth > MAX_DEPTH {
            return;
        }
        let Some(schema) = self.effective(schema) else {
            return;
        };
        if !schema.is_object() {
            return;
        }
        self.check_type(value, schema, path);
        self.check_enum(value, schema, path);
        self.check_number(value, schema, path);
        self.check_string(value, schema, path);
        self.check_object(value, schema, path);
        self.check_array(value, schema, path);
        self.check_combinators(value, schema, path);
    }

    /// Enforce `type`, naming the expected and found types in the mismatch message.
    fn check_type(&mut self, value: &Value, schema: &Value, path: &str) {
        let Some(expected) = schema.get("type").and_then(Value::as_str) else {
            return;
        };
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            other => {
                self.push(
                    path,
                    format!("unsupported schema type '{other}'"),
                    Severity::Warning,
                );
                return;
            }
        };
        if !matches {
            let found = type_name(value);
            self.push(
                path,
                format!("expected {expected}, found {found}"),
                Severity::Error,
            );
        }
    }

    /// Enforce `enum` and `const` with a readable list of the accepted values.
    fn check_enum(&mut self, value: &Value, schema: &Value, path: &str) {
        if let Some(expected) = schema.get("const") {
            if value != expected {
                self.push(
                    path,
                    format!("expected {}", render(expected)),
                    Severity::Error,
                );
            }
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            if !values.contains(value) {
                let allowed = values.iter().map(render).collect::<Vec<_>>().join(", ");
                self.push(path, format!("expected one of: {allowed}"), Severity::Error);
            }
        }
    }

    /// Enforce `minimum`, `maximum`, `exclusiveMinimum` and `multipleOf` on numbers.
    fn check_number(&mut self, value: &Value, schema: &Value, path: &str) {
        let Some(number) = value.as_f64() else {
            return;
        };
        if !number.is_finite() {
            self.push(path, "expected a finite number".into(), Severity::Error);
            return;
        }
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                self.push(path, format!("must be >= {minimum}"), Severity::Error);
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                self.push(path, format!("must be <= {maximum}"), Severity::Error);
            }
        }
        if let Some(minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
            if number <= minimum {
                self.push(path, format!("must be > {minimum}"), Severity::Error);
            }
        }
        if let Some(step) = schema.get("multipleOf").and_then(Value::as_f64) {
            if step > 0.0 && (number / step).fract().abs() > f64::EPSILON * 8.0 {
                self.push(
                    path,
                    format!("must be a multiple of {step}"),
                    Severity::Error,
                );
            }
        }
    }

    /// Enforce `minLength`, `maxLength` and `pattern` on strings.
    fn check_string(&mut self, value: &Value, schema: &Value, path: &str) {
        let Some(text) = value.as_str() else {
            return;
        };
        // Length limits count Unicode scalar values, matching the schema's own intent.
        let length = text.chars().count();
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if (length as u64) < minimum {
                self.push(
                    path,
                    format!("must be at least {minimum} character(s) long"),
                    Severity::Error,
                );
            }
        }
        if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64) {
            if (length as u64) > maximum {
                self.push(
                    path,
                    format!("must be at most {maximum} character(s) long"),
                    Severity::Error,
                );
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if !matches_pattern(text, pattern) {
                self.push(path, format!("must match {pattern}"), Severity::Error);
            }
        }
    }

    /// Enforce `required`, `properties`, `additionalProperties`, `patternProperties`,
    /// `propertyNames`, `maxProperties` and unknown-key reporting.
    fn check_object(&mut self, value: &Value, schema: &Value, path: &str) {
        let Some(object) = value.as_object() else {
            return;
        };
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    let child = join(path, name);
                    self.push(&child, "is required".into(), Severity::Error);
                }
            }
        }
        if let Some(maximum) = schema.get("maxProperties").and_then(Value::as_u64) {
            if object.len() as u64 > maximum {
                self.push(
                    path,
                    format!("must have at most {maximum} key(s)"),
                    Severity::Error,
                );
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        let patterns = schema.get("patternProperties").and_then(Value::as_object);
        let additional = schema.get("additionalProperties");
        for (key, entry) in object {
            let child = join(path, key);
            if let Some(subschema) = properties.and_then(|map| map.get(key)) {
                self.check(entry, subschema, &child);
            } else if let Some(subschema) = patterns.and_then(|map| {
                map.iter()
                    .find(|(pattern, _)| matches_pattern(key, pattern))
                    .map(|(_, schema)| schema)
            }) {
                self.check(entry, subschema, &child);
            } else {
                match additional {
                    Some(Value::Bool(false)) => {
                        let message = match known_path(key) {
                            Some(known) if known != child => {
                                format!("unknown setting '{child}'; did you mean '{known}'?")
                            }
                            _ => "unknown setting".to_owned(),
                        };
                        self.push(&child, message, Severity::Error);
                    }
                    Some(subschema @ Value::Object(_)) => {
                        self.check(entry, subschema, &child);
                    }
                    _ => {}
                }
            }
            if let Some(names) = schema.get("propertyNames") {
                self.check(&Value::String(key.clone()), names, &child);
            }
        }
    }

    /// Enforce `items`, `prefixItems`, `minItems` and `maxItems` on arrays.
    fn check_array(&mut self, value: &Value, schema: &Value, path: &str) {
        let Some(items) = value.as_array() else {
            return;
        };
        if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
            if (items.len() as u64) < minimum {
                self.push(
                    path,
                    format!("must have at least {minimum} item(s)"),
                    Severity::Error,
                );
            }
        }
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
            if (items.len() as u64) > maximum {
                self.push(
                    path,
                    format!("must have at most {maximum} item(s)"),
                    Severity::Error,
                );
            }
        }
        let prefix = schema
            .get("prefixItems")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        for (index, entry) in items.iter().enumerate() {
            let child = format!("{path}[{index}]");
            if let Some(subschema) = schema.get("items") {
                self.check(entry, subschema, &child);
            } else if index < prefix {
                if let Some(subschema) = schema
                    .get("prefixItems")
                    .and_then(Value::as_array)
                    .and_then(|list| list.get(index))
                {
                    self.check(entry, subschema, &child);
                }
            }
        }
    }

    /// Enforce `not`, `if`/`then`/`else`, and `allOf`/`anyOf`/`oneOf` membership.
    fn check_combinators(&mut self, value: &Value, schema: &Value, path: &str) {
        if let Some(negated) = schema.get("not") {
            if self.satisfies(value, negated) {
                self.push(
                    path,
                    "matches a shape this setting must not have".into(),
                    Severity::Error,
                );
            }
        }
        if let Some(condition) = schema.get("if") {
            let holds = self.satisfies(value, condition);
            let branch = if holds { "then" } else { "else" };
            if let Some(subschema) = schema.get(branch) {
                self.check(value, subschema, path);
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            let Some(branches) = schema.get(keyword).and_then(Value::as_array) else {
                continue;
            };
            if branches.is_empty() {
                continue;
            }
            if keyword == "allOf" {
                self.depth += 1;
                for branch in branches {
                    self.check(value, branch, path);
                }
                self.depth -= 1;
                continue;
            }
            let matched = branches
                .iter()
                .filter(|branch| self.problems(value, branch) == 0)
                .count();
            match (keyword, matched) {
                // Nothing matched: report the branch the value comes closest to, which names
                // the constraint actually broken instead of the first branch's type.
                (_, 0) => {
                    self.depth += 1;
                    self.check(value, self.closest_branch(value, branches), path);
                    self.depth -= 1;
                }
                ("oneOf", 1) => {}
                ("oneOf", found) => {
                    self.push(
                        path,
                        format!("matches {found} mutually exclusive shapes, expected 1"),
                        Severity::Error,
                    );
                }
                _ => {}
            }
        }
    }

    /// The branch the value was most likely aiming at: its declared `type` first, then the
    /// fewest unmet requirements. Schema order settles a remaining tie.
    fn closest_branch<'a>(&self, value: &Value, branches: &'a [Value]) -> &'a Value {
        let type_matches = |branch: &Value| {
            branch
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|declared| declared == json_type(value))
        };
        branches
            .iter()
            .min_by_key(|branch| (!type_matches(branch), self.problems(value, branch)))
            .unwrap_or(&branches[0])
    }

    /// How many requirements `value` breaks in `schema`, measured in a throwaway context.
    fn problems(&self, value: &Value, schema: &Value) -> usize {
        if self.depth > MAX_DEPTH {
            return 0;
        }
        let mut probe = Context {
            root: self.root,
            issues: Vec::new(),
            depth: self.depth + 1,
        };
        probe.check(value, schema, "");
        probe.issues.len()
    }

    /// Whether `value` satisfies `schema`, collecting problems in a throwaway context.
    fn satisfies(&self, value: &Value, schema: &Value) -> bool {
        if self.depth > MAX_DEPTH {
            return true;
        }
        let mut probe = Context {
            root: self.root,
            issues: Vec::new(),
            depth: self.depth + 1,
        };
        probe.check(value, schema, "");
        probe.issues.is_empty()
    }
}

/// A literal rendering of a schema keyword's expected value, for issue messages.
fn render(value: &Value) -> String {
    match value {
        Value::String(text) => format!("{text:?}"),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

/// The JSON type name shown in a mismatch message.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(number) if number.is_f64() && number.as_i64().is_none() => "a number",
        Value::Number(_) => "an integer",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The JSON type keyword matching a value, for comparing against a schema's `type`.
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Join a dotted parent path and a key, keeping the root path empty.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// One compiled schema pattern. Unsupported syntax degrades to a warning, never a panic.
enum Pattern {
    /// A pattern the `regex` crate can run.
    Compiled(regex::Regex),
    /// The schema's one lookahead pattern, matched by hand because `regex` has no lookahead.
    NonBlankNonUrl,
    /// A pattern using syntax `regex` rejects; matching is skipped rather than guessed.
    Unsupported,
}

/// The exact lookahead pattern the upstream schema uses, in `packs[].path` and `packs[].url`.
const NON_BLANK_NON_URL: &str = "^(?!\\s*$)(?!\\s*[hH][tT][tT][pP][sS]?://).+";

/// Match a schema `pattern` against a string, caching compiled patterns by source text.
fn matches_pattern(text: &str, pattern: &str) -> bool {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Pattern>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let Ok(mut cache) = cache.lock() else {
        return match compile(pattern) {
            Pattern::Compiled(regex) => regex.is_match(text),
            Pattern::NonBlankNonUrl => is_non_blank_non_url(text),
            Pattern::Unsupported => true,
        };
    };
    match cache
        .entry(pattern.to_owned())
        .or_insert_with(|| compile(pattern))
    {
        Pattern::Compiled(regex) => regex.is_match(text),
        Pattern::NonBlankNonUrl => is_non_blank_non_url(text),
        Pattern::Unsupported => true,
    }
}

/// `regex` has no lookahead, so the schema's one lookahead pattern is checked directly:
/// at least one non-blank character, and no `http://` or `https://` prefix after whitespace.
fn is_non_blank_non_url(text: &str) -> bool {
    let trimmed = text.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    !text.trim().is_empty() && !lower.starts_with("http://") && !lower.starts_with("https://")
}

/// Compile one schema pattern, mapping unsupported syntax to a documented fallback.
fn compile(pattern: &str) -> Pattern {
    if pattern == NON_BLANK_NON_URL {
        return Pattern::NonBlankNonUrl;
    }
    match regex::Regex::new(pattern) {
        Ok(regex) => Pattern::Compiled(regex),
        Err(_) => Pattern::Unsupported,
    }
}

/// Every dotted settings path the schema recognizes, sorted and deduplicated.
pub fn known_paths() -> Vec<String> {
    static PATHS: OnceLock<Vec<String>> = OnceLock::new();
    PATHS
        .get_or_init(|| {
            let mut paths = BTreeSet::new();
            collect_paths(schema(), "", &mut paths, 0);
            paths.into_iter().collect()
        })
        .clone()
}

/// Walk a subschema, recording the dotted path of every named setting it declares.
fn collect_paths(schema: &Value, path: &str, paths: &mut BTreeSet<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    // `$ref` resolves against the static schema root, so the borrow never aliases `paths`.
    let resolved = if schema.get("$ref").is_some() {
        schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| schema_root().pointer(reference.strip_prefix('#')?))
    } else {
        Some(schema)
    };
    let Some(schema) = resolved else {
        return;
    };
    if !schema.is_object() {
        return;
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        for branch in schema
            .get(keyword)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect_paths(branch, path, paths, depth + 1);
        }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, branch) in properties {
            let child = join(path, name);
            paths.insert(child.clone());
            collect_paths(branch, &child, paths, depth + 1);
        }
    }
    // `propertyNames` with an enum names the exact keys an open map accepts, as in
    // `shortcuts.bindings.<actionId>`; those are settings paths too.
    if let Some(names) = schema
        .get("propertyNames")
        .filter(|names| names.get("$ref").is_none())
    {
        if let Some(values) = names.get("enum").and_then(Value::as_array) {
            for name in values.iter().filter_map(Value::as_str) {
                paths.insert(join(path, name));
            }
        }
    }
    for branch in schema
        .get("patternProperties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|map| map.values())
    {
        collect_paths(branch, path, paths, depth + 1);
    }
}

/// The document root used by `collect_paths`; separated so the borrow stays short.
fn schema_root() -> &'static Value {
    schema()
}

/// The known path closest to `path`, for "did you mean" suggestions.
///
/// Candidates keep the same parent section when one is within tolerance, because a user typing
/// `sidebar.appearanc` means a sidebar setting. A tie is left unanswered rather than guessed at.
pub fn known_path(path: &str) -> Option<String> {
    let all = known_paths();
    if all.iter().any(|candidate| candidate == path) {
        return None;
    }
    let leaf = path.rsplit('.').next().unwrap_or(path);
    let parent = path.rfind('.').map_or("", |split| &path[..split]);
    // Prefer a same-section match, then fall back to the closest name anywhere in the schema.
    let in_section = |candidate: &str| {
        candidate
            .rsplit_once('.')
            .is_some_and(|(section, _)| section == parent)
    };
    nearest(&all, leaf, in_section).or_else(|| nearest(&all, leaf, |_| true))
}

/// The unique path whose last segment is the closest near-miss of `leaf`.
fn nearest(all: &[String], leaf: &str, section: impl Fn(&str) -> bool) -> Option<String> {
    let mut best: Option<(usize, &String)> = None;
    let mut tie = false;
    for candidate in all {
        let Some(candidate_leaf) = candidate.rsplit('.').next() else {
            continue;
        };
        if !section(candidate) {
            continue;
        }
        // Distance is measured on the last segment; the sections above it are too often shared.
        let distance = edit_distance(leaf, candidate_leaf);
        if distance == 0 || distance > MAX_SUGGESTION_DISTANCE {
            continue;
        }
        match best {
            Some((current, _)) if current < distance => {}
            Some((current, _)) if current == distance => tie = true,
            _ => {
                best = Some((distance, candidate));
                tie = false;
            }
        }
    }
    (!tie)
        .then(|| best.map(|(_, candidate)| candidate.clone()))
        .flatten()
}

/// Typo tolerance for a suggestion, in single-character edits.
const MAX_SUGGESTION_DISTANCE: usize = 2;

/// Levenshtein distance, bounded so a long path cannot cost more than the tolerance allows.
fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > MAX_SUGGESTION_DISTANCE {
        return MAX_SUGGESTION_DISTANCE + 1;
    }
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];
    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            current[column + 1] = substitution
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Look up a dotted settings path in a parsed document, returning `None` when absent.
pub fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Which settings file a document is read as.
///
/// macOS infers the same two scopes from the file it loaded: the user-wide file and the
/// project file discovered from the working directory. `--scope` forces the choice for a
/// path that sits nowhere in particular.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The user-wide `~/.config/cmux/cmux.json`.
    Global,
    /// A `cmux.json` in the working directory or one of its parents.
    Project,
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Scope::Global => formatter.write_str("global"),
            Scope::Project => formatter.write_str("project"),
        }
    }
}

/// Where a settings section may appear, read off the schema's own root description:
///
/// > Global cmux.json supports app settings, shortcuts, actions, custom commands, notification
/// > hooks, and workspace layouts. Project-local .cmux/cmux.json or cmux.json supports actions,
/// > commands, notification hooks, UI action wiring, Agent Chat overrides, vault agents,
/// > workspace-group overrides, and workspace launch/button configuration.
///
/// The schema carries no `scope` keyword, so this table is the only place the rule lives and
/// `known_paths()` stays the source of truth for which keys exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Availability {
    /// Only the user-wide file honors it.
    Global,
    /// Only a project file honors it.
    Project,
    /// Both files honor it, and the walk must descend to reach deeper single-scope sections.
    Both,
}

/// Section prefixes and the scope they belong to; the longest matching prefix wins.
///
/// Anything absent from the table is a global app setting, which is what the schema's
/// description says the rest of the top-level sections are.
const SCOPE_RULES: &[(&str, Availability)] = &[
    ("actions", Availability::Both),
    ("commands", Availability::Both),
    ("notifications.hooks", Availability::Both),
    ("notifications.hooksMode", Availability::Both),
    ("agentChat", Availability::Project),
    ("ui", Availability::Project),
    ("vault", Availability::Project),
    ("workspaceGroups", Availability::Project),
    ("newWorkspaceCommand", Availability::Project),
    ("surfaceTabBarButtons", Availability::Project),
];

/// The scope a dotted path belongs to, from the deepest matching rule.
fn availability(path: &str) -> Availability {
    SCOPE_RULES
        .iter()
        .filter(|(prefix, _)| path == *prefix || path.starts_with(&format!("{prefix}.")))
        .max_by_key(|(prefix, _)| prefix.len())
        .map_or(Availability::Global, |(_, value)| *value)
}

/// Whether a dotted path is meaningful in `scope`.
fn allows(path: &str, scope: Scope) -> bool {
    match availability(path) {
        Availability::Both => true,
        Availability::Global => scope == Scope::Global,
        Availability::Project => scope == Scope::Project,
    }
}

/// Report every setting that only means something in the other scope, as errors.
///
/// A section misplaced in a file is reported once, at the section, rather than once per leaf:
/// `sidebar.showPorts` and `sidebar.hideAllDetails` in a project file is one mistake, not two.
/// Sections that mix scopes, like `notifications`, are descended into so a project file can
/// still carry its `notifications.hooks` without dragging the rest of the section along.
pub fn scope_issues(value: &Value, scope: Scope) -> Vec<Issue> {
    fn walk(value: &Value, path: &str, scope: Scope, found: &mut Vec<Issue>) {
        match value {
            Value::Object(object) => {
                for (key, entry) in object {
                    let child = join(path, key);
                    if allows(&child, scope) {
                        continue;
                    }
                    if descends(entry, &child, scope) {
                        walk(entry, &child, scope, found);
                    } else {
                        let message = match availability(&child) {
                            Availability::Project => "only valid in a project cmux.json".into(),
                            _ => "only valid in the global cmux.json".into(),
                        };
                        found.push(Issue {
                            path: child,
                            message,
                            severity: Severity::Error,
                        });
                    }
                }
            }
            Value::Array(items) => {
                for (index, entry) in items.iter().enumerate() {
                    walk(entry, &format!("{path}[{index}]"), scope, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(value, "", scope, &mut found);
    found
}

/// Whether a misplaced section holds children the walk has to judge one by one.
///
/// Only true for a section that mixes scopes: the global `notifications` block, whose
/// `hooks` are honored in a project file while the rest of the block is not. A child that the
/// target scope rejects does not earn a descent; the section is reported whole instead.
fn descends(value: &Value, path: &str, scope: Scope) -> bool {
    let Value::Object(object) = value else {
        return false;
    };
    let here = availability(path);
    object.keys().any(|key| {
        let child = format!("{path}.{key}");
        availability(&child) != here && allows(&child, scope)
    })
}

/// Whether `path` is a dotted settings path with no empty segment.
fn is_path(path: &str) -> bool {
    !path.is_empty() && !path.contains("..") && !path.starts_with('.') && !path.ends_with('.')
}

/// Infer the scope of a settings file from its path, the way macOS infers it.
///
/// The global path wins outright. Otherwise a `cmux.json` that sits in the working directory
/// or one of its parents is a project file, matching the app's own project lookup. A file
/// that is neither is read as global, which is where `cmux config` points by default.
pub fn infer_scope(path: &Path) -> Scope {
    let directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    infer_scope_in(path, &directory)
}

/// [`infer_scope`] against an explicit working directory, so the rule is testable in place.
pub fn infer_scope_in(path: &Path, directory: &Path) -> Scope {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let normalized = lexical(&absolute);
    if global_path().is_some_and(|global| lexical(&absolute_path(&global)) == normalized) {
        return Scope::Global;
    }
    let Some(name) = normalized.file_name() else {
        return Scope::Global;
    };
    if name != "cmux.json" {
        return Scope::Global;
    }
    let Some(parent) = normalized.parent() else {
        return Scope::Global;
    };
    // `.cmux/cmux.json` is the project's preferred location, so the directory holding it is
    // the one the walk compares against.
    let parent = match parent.file_name() {
        Some(name) if name == ".cmux" => parent.parent().unwrap_or(parent),
        _ => parent,
    };
    let base = lexical(&absolute_path(directory));
    // The app walks at most 64 ancestors before giving up; the same bound applies here.
    base.ancestors()
        .take(64)
        .any(|ancestor| ancestor == parent)
        .then_some(Scope::Project)
        .unwrap_or(Scope::Global)
}

/// The global settings path, absolutized and normalized for comparison against a user path.
fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Drop `.` and `..` segments lexically, so two spellings of one path compare equal.
///
/// The file need not exist yet, so `canonicalize` is not an option: a `set` that creates
/// `cmux.json` still has to know which scope it is writing into.
fn lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// One byte range in the original JSONC text; both ends are exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Span {
    /// Byte offset of the first byte.
    start: usize,
    /// Byte offset one past the last byte.
    end: usize,
}

impl Span {
    /// The span's own extent, for use as a slice range.
    fn range(self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

/// One JSONC value kept as byte ranges, so an edit can be applied to the original text.
///
/// Nothing here re-serializes: the tree exists to say *where* a value is, and the bytes around
/// it, comments and all, are never rewritten.
#[derive(Debug)]
enum Node {
    /// A scalar, or any value whose contents are not navigated.
    Value(Span),
    /// An object, with the span of its braces and its members in document order.
    Object(Span, Vec<Member>),
    /// An array, with the span of its brackets and its items in document order.
    Array(Span, Vec<Node>),
}

impl Node {
    /// Byte offset of the first byte of this value.
    fn start(&self) -> usize {
        match self {
            Node::Value(span) => span.start,
            Node::Object(span, _) | Node::Array(span, _) => span.start,
        }
    }

    /// Byte offset just past this value.
    fn end(&self) -> usize {
        match self {
            Node::Value(span) => span.end,
            Node::Object(span, _) | Node::Array(span, _) => span.end,
        }
    }
}

/// One `"key": value` pair of a JSONC object.
#[derive(Debug)]
struct Member {
    /// Span of the key, quotes included.
    key: Span,
    /// The decoded key text, used to match a dotted path segment.
    name: String,
    /// The member value.
    node: Node,
}

/// Index of the next byte that carries meaning, skipping whitespace and comments.
fn skip_trivia(text: &str, classes: &[Class], mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len()
        && (bytes[at].is_ascii_whitespace()
            || matches!(
                classes.get(at),
                Some(Class::LineComment) | Some(Class::BlockComment)
            ))
    {
        at += 1;
    }
    at
}

/// End offset of the string literal whose opening quote is at `at`.
fn scan_string(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = at + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Decode a string literal, so an escaped key still matches its path segment.
fn decode_string(text: &str, span: Span) -> String {
    serde_json::from_str::<Value>(&text[span.range()])
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Parse the JSONC value starting at `at`, returning it and the offset just past it.
fn parse_node(text: &str, classes: &[Class], at: usize) -> Option<(Node, usize)> {
    let bytes = text.as_bytes();
    match *bytes.get(at)? {
        b'{' => parse_object(text, classes, at),
        b'[' => parse_array(text, classes, at),
        b'"' => {
            let end = scan_string(text, at)?;
            Some((Node::Value(Span { start: at, end }), end))
        }
        _ => {
            let mut end = at;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && !matches!(bytes[end], b',' | b'}' | b']')
                && !(bytes[end] == b'/' && matches!(bytes.get(end + 1), Some(b'/' | b'*')))
            {
                end += 1;
            }
            (end > at).then_some((Node::Value(Span { start: at, end }), end))
        }
    }
}

/// Parse a `{ … }` object, including the JSONC trailing comma.
fn parse_object(text: &str, classes: &[Class], at: usize) -> Option<(Node, usize)> {
    let mut members: Vec<Member> = Vec::new();
    let mut cursor = skip_trivia(text, classes, at + 1);
    if text.as_bytes().get(cursor) == Some(&b'}') {
        return Some((Node::Object(Span { start: at, end: cursor + 1 }, members), cursor + 1));
    }
    loop {
        if text.as_bytes().get(cursor) != Some(&b'"') {
            return None;
        }
        let key = Span {
            start: cursor,
            end: scan_string(text, cursor)?,
        };
        let name = decode_string(text, key);
        cursor = skip_trivia(text, classes, key.end);
        if text.as_bytes().get(cursor) != Some(&b':') {
            return None;
        }
        let value_at = skip_trivia(text, classes, cursor + 1);
        let (node, after) = parse_node(text, classes, value_at)?;
        members.push(Member { key, name, node });
        cursor = skip_trivia(text, classes, after);
        match text.as_bytes().get(cursor) {
            Some(b',') => {
                cursor = skip_trivia(text, classes, cursor + 1);
                if text.as_bytes().get(cursor) != Some(&b'}') {
                    continue;
                }
            }
            Some(b'}') => {}
            _ => return None,
        }
        return Some((Node::Object(Span { start: at, end: cursor + 1 }, members), cursor + 1));
    }
}

/// Parse a `[ … ]` array, including the JSONC trailing comma.
fn parse_array(text: &str, classes: &[Class], at: usize) -> Option<(Node, usize)> {
    let mut items: Vec<Node> = Vec::new();
    let mut cursor = skip_trivia(text, classes, at + 1);
    if text.as_bytes().get(cursor) == Some(&b']') {
        return Some((Node::Array(Span { start: at, end: cursor + 1 }, items), cursor + 1));
    }
    loop {
        let (node, after) = parse_node(text, classes, cursor)?;
        items.push(node);
        cursor = skip_trivia(text, classes, after);
        match text.as_bytes().get(cursor) {
            Some(b',') => {
                cursor = skip_trivia(text, classes, cursor + 1);
                if text.as_bytes().get(cursor) != Some(&b']') {
                    continue;
                }
            }
            Some(b']') => {}
            _ => return None,
        }
        return Some((Node::Array(Span { start: at, end: cursor + 1 }, items), cursor + 1));
    }
}

/// Parse a whole document, returning `None` when it is empty, blank or not JSONC.
fn parse_document(text: &str) -> Option<Node> {
    let classes = classify(text).ok()?;
    let at = skip_trivia(text, classes.as_slice(), 0);
    if at >= text.len() {
        return None;
    }
    let (node, after) = parse_node(text, classes.as_slice(), at)?;
    (skip_trivia(text, classes.as_slice(), after) == text.len()).then_some(node)
}

/// The syntax error behind a document the structural walk could not read.
///
/// The scanner is deliberately stricter than `serde_json` in one direction only: it cannot
/// recover from a malformed node, so the JSONC parser's own diagnosis is the one to show.
fn unreadable(text: &str) -> EditError {
    EditError::Syntax(parse_jsonc(text).err().map_or_else(
        || "the file is not a JSONC document".into(),
        |error| error.to_string(),
    ))
}

/// Whether any byte of `span` sits in a comment.
fn has_comment(text: &str, classes: &[Class], span: Span) -> bool {
    text.as_bytes()[span.range()].iter().enumerate().any(|(offset, _)| {
        matches!(
            classes.get(span.start + offset),
            Some(Class::LineComment) | Some(Class::BlockComment)
        )
    })
}

/// The leading whitespace of the line holding `offset`, or empty when the line holds more.
fn line_indent(text: &str, offset: usize) -> String {
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let line = &text[start..offset];
    line.chars()
        .all(|character| character == ' ' || character == '\t')
        .then(|| line.to_owned())
        .unwrap_or_default()
}

/// The indentation step the file already uses, taken from its shallowest indented line.
///
/// Four spaces is the fallback, matching the examples in the settings documentation.
fn indent_step(text: &str) -> usize {
    text.lines()
        .filter_map(|line| {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            (spaces > 0).then_some(spaces)
        })
        .min()
        .unwrap_or(4)
}

/// Where a dotted path meets the document: an existing member, or the object to extend.
enum Located<'a> {
    /// The path names an object member that is already there; replace its value.
    Existing(&'a Member),
    /// The path names an array element that is already there; replace it.
    Element(&'a Node),
    /// The path is missing below `container`; `rest` are the segments still to create.
    Missing {
        /// The innermost node the new member would go into.
        container: &'a Node,
        /// Path segments after that node, still to be created.
        rest: &'a [&'a str],
    },
}

/// Walk `segments` from the document root to the member a `set` would replace or extend.
fn locate<'a>(node: &'a Node, segments: &'a [&'a str]) -> Located<'a> {
    let Some((head, rest)) = segments.split_first() else {
        return Located::Missing {
            container: node,
            rest: &[],
        };
    };
    match node {
        Node::Object(_, members) => match members.iter().find(|member| member.name == *head) {
            Some(member) if rest.is_empty() => Located::Existing(member),
            Some(member) => locate(&member.node, rest),
            None => Located::Missing {
                container: node,
                rest: segments,
            },
        },
        Node::Array(_, items) => match head.parse::<usize>().ok().and_then(|index| items.get(index)) {
            Some(item) if rest.is_empty() => Located::Element(item),
            Some(item) => locate(item, rest),
            // An array entry is never conjured: inserting one would renumber its siblings.
            None => Located::Missing {
                container: node,
                rest: segments,
            },
        },
        Node::Value(_) => Located::Missing {
            container: node,
            rest: segments,
        },
    }
}

/// Render `"key": value` for `segments`, nesting the objects a missing path still needs.
fn render_entry(
    segments: &[&str],
    value: &Value,
    indent: &str,
    step: usize,
) -> Result<String, EditError> {
    let encoded = encode(value)?;
    let Some((head, rest)) = segments.split_first() else {
        return Ok(encoded);
    };
    let key = serde_json::to_string(head)
        .map_err(|error| EditError::NotAnObject(error.to_string()))?;
    if rest.is_empty() {
        return Ok(format!("{key}: {encoded}"));
    }
    let inner = format!("{indent}{}", " ".repeat(step));
    let body = render_entry(rest, value, &inner, step)?;
    Ok(format!("{key}: {{\n{inner}{body}\n{indent}}}"))
}

/// A targeted text edit could not be applied without risking the user's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The file is not JSONC, so nothing about its layout can be preserved.
    Syntax(String),
    /// The path does not reach an object: a segment names a scalar, or an array index.
    NotAnObject(String),
    /// The text to replace carries comments; the edit is refused rather than dropping them.
    CommentsInValue(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::Syntax(message)
            | EditError::NotAnObject(message)
            | EditError::CommentsInValue(message) => formatter.write_str(message),
        }
    }
}

/// What a write did to the file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WriteStatus {
    /// The file was rewritten and now holds the change.
    Persisted,
    /// The key was already absent, so the file was left alone.
    Unchanged,
}

/// The result of a `set` or `unset`, as the CLI reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteOutcome {
    /// Whether the file changed.
    pub status: WriteStatus,
    /// The dotted path that was written.
    pub key: String,
}

/// A write that would have broken the file, refused before touching it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The targeted edit could not be applied safely.
    Edit(EditError),
    /// The change would add these errors to the file, so nothing was written.
    Invalid(Vec<Issue>),
    /// The file could not be read or written.
    Io(String),
}

impl From<EditError> for ApplyError {
    fn from(error: EditError) -> Self {
        ApplyError::Edit(error)
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::Edit(error) => write!(formatter, "{error}"),
            ApplyError::Invalid(issues) => {
                write!(formatter, "the change would add {} error(s)", issues.len())
            }
            ApplyError::Io(message) => formatter.write_str(message),
        }
    }
}

/// Read one settings file as text, treating an absent file as an empty document.
pub fn read_text(path: &Path) -> Result<String, String> {
    let Some(bytes) = read_source(path)? else {
        return Ok(String::new());
    };
    String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// Set `key` to `value`, or remove it when `value` is `None`, preserving the rest of the file.
///
/// The file is edited as text, so comments, key order and layout outside the changed key stay
/// exactly as the user wrote them. The result is validated before anything is written: when the
/// change would add an error, the file is left untouched. Errors the file already had never
/// block an unrelated change, so a key from a newer cmux does not make the file unwritable.
pub fn apply(
    path: &Path,
    key: &str,
    value: Option<&Value>,
    scope: Scope,
) -> Result<WriteOutcome, ApplyError> {
    if !is_path(key) {
        return Err(ApplyError::Edit(EditError::NotAnObject(format!(
            "'{key}' is not a dotted settings path"
        ))));
    }
    let segments: Vec<&str> = key.split('.').collect();
    let original = read_text(path).map_err(ApplyError::Io)?;
    let before = document(&original).map_err(ApplyError::Edit)?;
    let before_issues = issues(&before, scope);
    let edited = match value {
        Some(value) => set_text(&original, &segments, value)?,
        None => match remove_text(&original, &segments)? {
            Some(text) => text,
            None => {
                return Ok(WriteOutcome {
                    status: WriteStatus::Unchanged,
                    key: key.to_owned(),
                })
            }
        },
    };
    let after = document(&edited).map_err(ApplyError::Edit)?;
    let introduced: Vec<Issue> = issues(&after, scope)
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error && !before_issues.contains(issue))
        .collect();
    if !introduced.is_empty() {
        return Err(ApplyError::Invalid(introduced));
    }
    write_atomic(path, &edited).map_err(ApplyError::Io)?;
    Ok(WriteOutcome {
        status: WriteStatus::Persisted,
        key: key.to_owned(),
    })
}

/// Every problem in a document, schema findings and misplaced-scope settings alike.
fn issues(value: &Value, scope: Scope) -> Vec<Issue> {
    let mut found = validate(value);
    found.extend(scope_issues(value, scope));
    found
}

/// Parse a document for editing, treating blank text as an empty object.
/// Parse a document for editing, treating blank text as an empty object.
///
/// A file holding nothing but comments is empty JSONC, not a syntax error, so the first write
/// into it still works instead of refusing a change the user plainly meant to make.
fn document(text: &str) -> Result<Value, EditError> {
    if is_blank(text) {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    parse_jsonc(text).map_err(|error| EditError::Syntax(error.to_string()))
}

/// Whether the text holds no JSONC value at all, only whitespace and comments.
fn is_blank(text: &str) -> bool {
    let Ok(classes) = classify(text) else {
        return false;
    };
    skip_trivia(text, &classes, 0) >= text.len()
}

/// Replace the value at `segments`, or create the objects and member a missing path needs.
fn set_text(text: &str, segments: &[&str], value: &Value) -> Result<String, EditError> {
    if is_blank(text) {
        let step = indent_step(text);
        let indent = " ".repeat(step);
        let body = render_entry(segments, value, &indent, step)?;
        return Ok(format!("{{\n{indent}{body}\n}}\n"));
    }
    let root = parse_document(text).ok_or_else(|| unreadable(text))?;
    match locate(&root, segments) {
        // Only the value is rewritten; the key, its spacing and the comments around it stay.
        Located::Existing(member) => {
            let target = Span {
                start: member.node.start(),
                end: member.node.end(),
            };
            let guard = Span {
                start: member.key.start,
                end: target.end,
            };
            replace_span(text, guard, target, segments, &encode(value)?)
        }
        Located::Element(node) => {
            let span = Span {
                start: node.start(),
                end: node.end(),
            };
            replace_span(text, span, span, segments, &encode(value)?)
        }
        Located::Missing { container, rest } => insert_path(text, container, rest, value),
    }
}

/// Encode a value the way it will be written into the file.
fn encode(value: &Value) -> Result<String, EditError> {
    serde_json::to_string(value)
        .map_err(|error| EditError::NotAnObject(format!("cannot encode the value: {error}")))
}

/// Replace `target` with `encoded`, after refusing when the member carries comments.
///
/// `guard` covers the key as well as the value: a comment between the two is as much the
/// user's text as one inside the value, and neither is worth losing to make a `set` succeed.
fn replace_span(
    text: &str,
    guard: Span,
    target: Span,
    segments: &[&str],
    encoded: &str,
) -> Result<String, EditError> {
    let classes = classify(text).map_err(|error| EditError::Syntax(error.to_string()))?;
    if has_comment(text, &classes, guard) {
        return Err(EditError::CommentsInValue(format!(
            "'{}' carries comments; edit it by hand to keep them",
            segments.join(".")
        )));
    }
    let mut out = String::with_capacity(text.len() + encoded.len());
    out.push_str(&text[..target.start]);
    out.push_str(encoded);
    out.push_str(&text[target.end..]);
    Ok(out)
}

/// Add `rest` under `container` with `value` at its end, creating objects as needed.
fn insert_path(
    text: &str,
    container: &Node,
    rest: &[&str],
    value: &Value,
) -> Result<String, EditError> {
    let Node::Object(span, members) = container else {
        return Err(EditError::NotAnObject(format!(
            "'{}' is not an object",
            rest.first().copied().unwrap_or("")
        )));
    };
    let step = indent_step(text);
    let body = &text[span.range()];
    if members.is_empty() {
        let entry = render_entry(rest, value, "", step)?;
        let mut out = String::with_capacity(text.len() + entry.len() + 4);
        out.push_str(&text[..span.start + 1]);
        out.push_str(&entry);
        out.push_str(&text[span.end - 1..]);
        return Ok(out);
    }
    if !body.contains('\n') {
        let entry = render_entry(rest, value, "", step)?;
        let mut out = String::with_capacity(text.len() + entry.len() + 4);
        out.push_str(&text[..span.end - 1]);
        out.push_str(&format!(", {entry}"));
        out.push_str(&text[span.end - 1..]);
        return Ok(out);
    }
    let last = members.last().expect("a non-empty object has a last member");
    let inner = line_indent(text, last.key.start);
    let trailing = text.as_bytes()[last.node.end()..span.end - 1]
        .iter()
        .skip_while(|byte| byte.is_ascii_whitespace())
        .next()
        == Some(&b',');
    let entry = render_entry(rest, value, &inner, step)?;
    // Insert before the whitespace that indents the closing brace, so it keeps its own line.
    let close = span.end - 1;
    let insert_at = close - (text[last.node.end()..close].len()
        - text[last.node.end()..close].trim_end_matches([' ', '\t', '\n', '\r']).len());
    let mut out = String::with_capacity(text.len() + entry.len() + inner.len() + 2);
    out.push_str(&text[..insert_at]);
    if !trailing {
        out.push(',');
    }
    out.push('\n');
    out.push_str(&inner);
    out.push_str(&entry);
    out.push_str(&text[insert_at..]);
    Ok(out)
}

/// Remove the member at `segments`, returning `None` when the key is already absent.
fn remove_text(text: &str, segments: &[&str]) -> Result<Option<String>, EditError> {
    if is_blank(text) {
        return Ok(None);
    }
    let classes = classify(text).map_err(|error| EditError::Syntax(error.to_string()))?;
    let root = parse_document(text).ok_or_else(|| unreadable(text))?;
    let member = match locate(&root, segments) {
        Located::Existing(member) => member,
        // An array element and an absent key are both already "nothing to remove".
        Located::Element(_) | Located::Missing { .. } => return Ok(None),
    };
    let mut start = member.key.start;
    let end = member.node.end();
    if has_comment(text, &classes, Span { start, end }) {
        return Err(EditError::CommentsInValue(format!(
            "'{}' carries comments; edit it by hand to keep them",
            segments.join(".")
        )));
    }
    let mut after = end;
    let bytes = text.as_bytes();
    while matches!(bytes.get(after), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        after += 1;
    }
    if bytes.get(after) == Some(&b',') {
        after += 1;
    }
    // When the member owns the rest of its line, the line goes with it: either nothing but
    // whitespace is left, or a comment that described the member being removed. Its leading
    // indentation goes too, so the next member is not left padded by the dead line.
    let line_end = text[after..]
        .find('\n')
        .map_or(text.len(), |offset| after + offset);
    let rest = &text[after..line_end];
    if rest.trim().is_empty() || rest.contains("//") || rest.contains("/*") {
        after = if line_end < text.len() { line_end + 1 } else { text.len() };
        let line_start = text[..start].rfind('\n').map_or(0, |offset| offset + 1);
        if text[line_start..start]
            .chars()
            .all(|character| matches!(character, ' ' | '\t'))
        {
            start = line_start;
        }
    }
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[after..]);
    Ok(Some(out))
}

/// Replace a file through a sibling temporary file and one `rename`.
///
/// The rename is atomic within a directory, so a reader sees either the whole old file or the
/// whole new one, never a half-written settings file that fails to parse.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let directory = match path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        Some(parent) => parent,
        None => Path::new("."),
    };
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cmux.json".into());
    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, text)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    // Keep the mode the file already had; a file created here follows the process umask.
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temporary, metadata.permissions());
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}

/// The upstream schema URL, the authority for keys, types and bounds.
pub const SCHEMA_URL: &str = "https://raw.githubusercontent.com/manaflow-ai/cmux/main/web/data/cmux.schema.json";

/// The rendered settings documentation, mirroring the schema one for one.
pub const DOCS_URL: &str = "https://cmux.com/docs/configuration";

/// Curated examples for `cmux config docs`, mirroring the settings skill's quick reference.
const DOC_EXAMPLES: &[(&str, &str)] = &[
    ("app.appearance", r#""system" | "light" | "dark""#),
    ("app.accentColor", r#""cmux" | "system""#),
    ("sidebarAppearance.tintOpacity", "0..1"),
    ("sidebar.showPorts", "boolean"),
    ("notifications.sound", r#""none" | "custom_file" | a system sound name"#),
    (
        "automation.socketControlMode",
        r#""off" | "cmuxOnly" | "automation" | "password" | "allowAll""#),
    ("markdown.fontSize", "8..72"),
    (
        "fileExplorer.doubleClickAction",
        r#""preview" | "open" | "preferredEditor""#),
    ("shortcuts.bindings.<actionId>", r#""cmd+b" | ["ctrl+b","c"] | null | """#),
];

/// The top-level sections of the schema, split by the scope that honors them.
fn sections() -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut global = Vec::new();
    let mut shared = Vec::new();
    let mut project = Vec::new();
    let Some(properties) = schema().get("properties").and_then(Value::as_object) else {
        return (global, shared, project);
    };
    for name in properties.keys() {
        match availability(name) {
            Availability::Global => global.push(name.clone()),
            Availability::Both => shared.push(name.clone()),
            Availability::Project => project.push(name.clone()),
        }
    }
    (global, shared, project)
}

/// The settings reference printed by `cmux config docs`.
///
/// It names where the files live, what the format accepts, which sections each scope honors and
/// a few real keys, so a first `cmux config set` does not need the full documentation open.
pub fn docs_text() -> String {
    let (global, shared, project) = sections();
    let global_path = global_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unset: XDG_CONFIG_HOME or HOME is missing".into());
    let wrap = |list: &[String]| list.join(", ");
    format!(
        "cmux.json settings\n\
         \n\
         Files:\n\
         \x20 global:  {global_path}\n\
         \x20 project: .cmux/cmux.json or cmux.json in the current directory or a parent\n\
         \x20 format:  JSONC, so // and /* */ comments and trailing commas are accepted\n\
         \n\
         Scopes:\n\
         \x20 global only: {}\n\
         \x20 both:        {}\n\
         \x20 project only: {}\n\
         A setting that belongs to one scope and sits in the other is an error, so\n\
         --scope global|project tells `validate` which file it is looking at.\n\
         \n\
         Commands:\n\
         \x20 cmux config path                              print the global cmux.json path\n\
         \x20 cmux config validate [--file F] [--scope S] check the file against the schema\n\
         \x20 cmux config get <a.b.c> [--file F]           print the value at a dotted path\n\
         \x20 cmux config set <a.b.c> <json> [--file F]    set a value, keeping comments\n\
         \x20 cmux config unset <a.b.c> [--file F]         remove a value\n\
         \x20 cmux config list-supported                    every path the schema recognizes\n\
         \x20 cmux config docs                              this reference\n\
         \n\
         Values are JSON: true, 12, \"dark\", [\"a\",\"b\"]. An unquoted word is stored as a string.\n\
         `set` and `unset` edit the file as text, keep its comments, and write nothing when the\n\
         change would add a validation error. A running cmux applies the file on save; this\n\
         command never contacts it.\n\
         \n\
         Examples:\n\
         {}\n\
         \n\
         Schema: {SCHEMA_URL}\n\
         Docs:   {DOCS_URL}\n",
        wrap(&global),
        wrap(&shared),
        wrap(&project),
        DOC_EXAMPLES
            .iter()
            .map(|(path, shape)| format!("  {path:<38}{shape}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The same reference as a JSON object, for `cmux config docs --json`.
pub fn docs_json() -> Value {
    let (global, shared, project) = sections();
    serde_json::json!({
        "global_path": global_path().map(|path| path.display().to_string()),
        "project_paths": [".cmux/cmux.json", "cmux.json"],
        "format": "jsonc",
        "sections": {
            "global": global,
            "shared": shared,
            "project": project,
        },
        "commands": [
            "cmux config path",
            "cmux config validate [--file F] [--scope global|project]",
            "cmux config get <a.b.c> [--file F]",
            "cmux config set <a.b.c> <json> [--file F] [--scope global|project]",
            "cmux config unset <a.b.c> [--file F] [--scope global|project]",
            "cmux config list-supported",
            "cmux config docs",
        ],
        "examples": DOC_EXAMPLES
            .iter()
            .map(|(path, shape)| serde_json::json!({"path": path, "shape": shape}))
            .collect::<Vec<_>>(),
        "known_paths": known_paths().len(),
        "schema_url": SCHEMA_URL,
        "docs_url": DOCS_URL,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic config assembled from the examples in `cmux-settings-SKILL.md`, exercising
    /// comments, trailing commas, nested objects, enums, bounds, arrays and shortcut bindings.
    const REALISTIC: &str = r##"{
        // cmux user settings, written by hand and validated with `cmux config validate`.
        "schemaVersion": 1,
        "app": {
            "appearance": "dark",
            "accentColor": "cmux",
            "minimalMode": true,
            "globalFontMagnification": 110,
            "confirmQuit": "dirty-only",
            "defaultWorkspacePath": "~/code",
        },
        "sidebarAppearance": {
            "matchTerminalBackground": true,
            "tintColor": "#101014",
            "tintOpacity": 0.18,
        },
        "sidebar": {
            "hideAllDetails": false,
            "showPorts": true,
            "branchLayout": "inline",
            "notificationMessageLineLimit": 3,
        },
        "notifications": {
            "sound": "Ping",
            "agentTurnComplete": "whenIdle",
            "soundOverrides": {
                "codex": { "turnDone": { "sound": "none" } },
            },
        },
        "automation": {
            "socketControlMode": "cmuxOnly",
            "portBase": 9100,
            "portRange": 10,
        },
        "browser": {
            "defaultSearchEngine": "duckduckgo",
            "hostsToOpenInEmbeddedBrowser": ["localhost", "*.internal.example"],
        },
        "markdown": { "fontSize": 16 },
        "fileEditor": { "wordWrap": true },
        "fileExplorer": { "doubleClickAction": "preferredEditor" },
        "diffViewer": { "defaultLayout": "split" },
        "shortcuts": {
            "bindings": {
                "newTab": ["ctrl+b", "c"],
                "commandPalette": "ctrl+p",
            },
        },
        /* Project actions are hand-tuned and share this file. */
        "actions": { "launch": { "type": "workspaceCommand", "name": "dev" } },
        "commands": [{ "name": "dev", "workspace": { "cwd": "." } }],
    }"##;

    /// Line comments, block comments and trailing commas are all removed outside strings.
    #[test]
    fn parses_jsonc_comments_and_trailing_commas() {
        let value = parse_jsonc(
            r#"{
                // a line comment
                "a": 1, /* inline block */
                "b": [1, 2, 3,],
                "c": {"d": true,},
            }"#,
        )
        .unwrap();
        assert_eq!(value["a"], 1);
        assert_eq!(value["b"], serde_json::json!([1, 2, 3]));
        assert_eq!(value["c"]["d"], true);
    }

    /// `//` inside a string is data, not a comment, and survives parsing intact.
    #[test]
    fn comment_markers_inside_strings_are_preserved() {
        let value = parse_jsonc(
            r#"{
                "url": "http://example.com/path",
                "regex": "/* not a comment */",
                "trailing": "value, still a string",
                "escaped": "say \"hi\" // now a comment marker",
            }"#,
        )
        .unwrap();
        assert_eq!(value["url"], "http://example.com/path");
        assert_eq!(value["regex"], "/* not a comment */");
        assert_eq!(value["trailing"], "value, still a string");
        assert_eq!(value["escaped"], "say \"hi\" // now a comment marker");
    }

    /// A syntax error reports the line and column of the original text, comment offsets included.
    #[test]
    fn syntax_error_reports_original_line_and_column() {
        // A missing comma between two members: the error sits on the line that opens the member.
        let error = parse_jsonc("{\n  \"a\": 1,\n  \"b\": 2\n  \"c\": 3\n}").unwrap_err();
        assert_eq!(error.line, 4);
        assert!(error.to_string().starts_with("4:"), "{error}");

        // Comments are blanked, never deleted, so a failing document reports the same
        // position as the identical document written without them.
        let with_comments =
            parse_jsonc("{\n  // one\n  /* two */\n  \"a\": 1,\n  \"b\": 2\n  \"c\": 3\n}")
                .unwrap_err();
        assert_eq!(with_comments.line, error.line + 2);
        assert_eq!(with_comments.column, error.column);

        let unterminated = parse_jsonc("{\n  \"a\": \"open\n}").unwrap_err();
        assert_eq!(unterminated.line, 2);
        assert!(unterminated.message.contains("string"), "{unterminated}");

        let block = parse_jsonc("{\n /* never closed\n \"a\": 1\n}").unwrap_err();
        assert_eq!(block.line, 2);
        assert!(block.message.contains("block comment"), "{block}");
    }

    /// An unknown top-level key is an error addressed by its dotted path.
    #[test]
    fn unknown_key_is_reported_with_a_suggestion() {
        let issues = validate(&parse_jsonc(r#"{"app": {"appearanc": "dark"}}"#).unwrap());
        let unknown = issues
            .iter()
            .find(|issue| issue.path == "app.appearanc")
            .expect("unknown key reported");
        assert_eq!(unknown.severity, Severity::Error);
        assert!(
            unknown.message.contains("did you mean 'app.appearance'"),
            "{}",
            unknown.message
        );
    }

    /// A wrong JSON type is an error; the message names both the expected and found types.
    #[test]
    fn wrong_type_is_reported() {
        let issues = validate(&parse_jsonc(r#"{"sidebar": {"showPorts": "yes"}}"#).unwrap());
        let wrong = issues
            .iter()
            .find(|issue| issue.path == "sidebar.showPorts")
            .expect("type mismatch reported");
        assert_eq!(wrong.severity, Severity::Error);
        assert_eq!(wrong.message, "expected boolean, found a string");
    }

    /// A value outside the schema's enum lists the accepted values.
    #[test]
    fn violated_enum_lists_allowed_values() {
        let issues =
            validate(&parse_jsonc(r#"{"sidebar": {"branchLayout": "diagonal"}}"#).unwrap());
        let bad = issues
            .iter()
            .find(|issue| issue.path == "sidebar.branchLayout")
            .expect("enum violation reported");
        assert_eq!(bad.severity, Severity::Error);
        assert_eq!(bad.message, "expected one of: \"vertical\", \"inline\"");
    }

    /// `minimum` bounds are enforced, and a value inside the bound is accepted.
    #[test]
    fn minimum_bound_is_enforced() {
        let issues = validate(&parse_jsonc(r#"{"markdown": {"fontSize": 4}}"#).unwrap());
        let low = issues
            .iter()
            .find(|issue| issue.path == "markdown.fontSize")
            .expect("minimum reported");
        assert_eq!(low.severity, Severity::Error);
        assert_eq!(low.message, "must be >= 8");

        let ok = validate(&parse_jsonc(r#"{"markdown": {"fontSize": 16}}"#).unwrap());
        assert!(
            ok.iter().all(|issue| issue.severity != Severity::Error),
            "{ok:?}"
        );
    }

    /// A `$ref` into `$defs` is resolved, so shared definitions are validated too.
    #[test]
    fn reference_definitions_are_resolved() {
        // `sidebar.workspaceDescriptionColor` is `$ref` to `colorHexOrNull`.
        let good = validate(
            &parse_jsonc(r##"{"sidebar": {"workspaceDescriptionColor": "#AABBCC"}}"##).unwrap(),
        );
        assert!(
            good.iter().all(|issue| issue.severity != Severity::Error),
            "{good:?}"
        );

        let issues =
            validate(&parse_jsonc(r#"{"sidebar": {"workspaceDescriptionColor": "red"}}"#).unwrap());
        let bad = issues
            .iter()
            .find(|issue| issue.path == "sidebar.workspaceDescriptionColor")
            .expect("$ref pattern applied");
        assert_eq!(bad.severity, Severity::Error);
    }

    /// A value satisfying no `oneOf` branch reports that branch's requirements.
    #[test]
    fn one_of_reports_the_first_branch_requirements() {
        let issues =
            validate(&parse_jsonc(r#"{"fileExplorer": {"doubleClickAction": "open"}}"#).unwrap());
        let bad = issues
            .iter()
            .find(|issue| issue.path == "fileExplorer.doubleClickAction")
            .expect("oneOf violation reported");
        assert_eq!(bad.severity, Severity::Error);
        assert!(
            bad.message.contains("preview"),
            "expected the first branch's enum, got {}",
            bad.message
        );
    }

    /// A value satisfying no `oneOf` branch names the constraint it broke, not the first
    /// branch's type: `packs[].path` is an object branch and a string branch, and a blank
    /// path must be reported against the object form the author actually wrote.
    #[test]
    fn one_of_diagnostic_names_the_broken_constraint() {
        let issues = validate(&parse_jsonc(r#"{"packs": [{"path": "  "}]}"#).unwrap());
        let bad = issues
            .iter()
            .find(|issue| issue.path == "packs[0].path")
            .expect("the blank path itself is reported");
        assert_eq!(bad.severity, Severity::Error);
        assert!(
            bad.message.starts_with("must match"),
            "expected the pattern failure, got {}",
            bad.message
        );

        // A value of no branch's type still falls back to the first branch's type message.
        let issues = validate(&parse_jsonc(r#"{"packs": [true]}"#).unwrap());
        let bad = issues
            .iter()
            .find(|issue| issue.path == "packs[0]")
            .expect("packs item reported");
        assert_eq!(bad.message, "expected string, found a boolean");
    }

    /// macOS-only keys warn instead of failing, while remaining keys in the file still error.
    #[test]
    fn macos_only_keys_warn_instead_of_failing() {
        let value = parse_jsonc(
            r#"{
                "app": {"appIcon": "dark", "menuBarOnly": true, "appearanc": "dark"},
                "notifications": {"dockBadge": true},
                "computerUse": {"showInMenuBar": false},
                "mobile": {"artifactFolderAccess": true},
            }"#,
        )
        .unwrap();
        let issues = validate(&value);
        for key in [
            "app.appIcon",
            "app.menuBarOnly",
            "notifications.dockBadge",
            "computerUse.showInMenuBar",
            "mobile.artifactFolderAccess",
        ] {
            let issue = issues
                .iter()
                .find(|issue| issue.path == key)
                .unwrap_or_else(|| panic!("{key} reported"));
            assert_eq!(issue.severity, Severity::Warning, "{key}");
            assert!(
                issue.message.contains("unsupported on Linux"),
                "{}",
                issue.message
            );
        }
        // A real mistake elsewhere in the same file is still an error.
        assert_eq!(Issue::errors(&issues), 1);
        assert!(issues
            .iter()
            .any(|issue| issue.path == "app.appearanc" && issue.severity == Severity::Error));
    }

    /// Every problem in a file is listed, not just the first one found.
    #[test]
    fn all_problems_are_reported_not_just_the_first() {
        let issues = validate(
            &parse_jsonc(
                r#"{
                    "sidebar": {"showPorts": "yes", "branchLayout": "diagonal"},
                    "markdown": {"fontSize": 2},
                    "nonsense": true,
                }"#,
            )
            .unwrap(),
        );
        for path in [
            "sidebar.showPorts",
            "sidebar.branchLayout",
            "markdown.fontSize",
            "nonsense",
        ] {
            assert!(
                issues.iter().any(|issue| issue.path == path),
                "{path} missing from {issues:?}"
            );
        }
    }

    /// `get` walks dotted paths through objects and array indices, and misses cleanly.
    #[test]
    fn get_walks_nested_paths() {
        let value = parse_jsonc(
            r#"{"sidebar": {"branchLayout": "inline"}, "hosts": ["a", "b"], "n": {"deep": {"x": 7}}}"#,
        )
        .unwrap();
        assert_eq!(
            get(&value, "sidebar.branchLayout"),
            Some(&Value::from("inline"))
        );
        assert_eq!(get(&value, "hosts.1"), Some(&Value::from("b")));
        assert_eq!(get(&value, "n.deep.x"), Some(&Value::from(7)));
        assert_eq!(get(&value, "sidebar.missing"), None);
        assert_eq!(get(&value, "n.deep.x.deeper"), None);
        assert_eq!(get(&value, "hosts.9"), None);
    }

    /// A realistic configuration built from the skill's examples validates without any error.
    #[test]
    fn realistic_configuration_validates_cleanly() {
        let value = parse_jsonc(REALISTIC).expect("realistic config parses as JSONC");
        let issues = validate(&value);
        assert_eq!(Issue::errors(&issues), 0, "unexpected errors: {issues:?}");
        assert!(
            issues
                .iter()
                .all(|issue| issue.severity == Severity::Warning),
            "unexpected warnings: {issues:?}"
        );
        assert_eq!(
            get(&value, "shortcuts.bindings.newTab.1"),
            Some(&Value::from("c"))
        );
    }

    /// The bundled schema loads whole, and its paths cover more than the documented 100 keys.
    #[test]
    fn bundled_schema_loads_and_lists_known_paths() {
        let root = schema();
        assert_eq!(
            root["$id"],
            "https://raw.githubusercontent.com/manaflow-ai/cmux/main/web/data/cmux.schema.json"
        );
        assert!(root["properties"].as_object().unwrap().len() > 20);
        assert!(root["$defs"]["colorHex"].is_object());

        let paths = known_paths();
        assert!(paths.len() > 100, "only {} known paths", paths.len());
        for expected in [
            "sidebar.branchLayout",
            "app.appearance",
            "terminal.scrollSpeed",
            "shortcuts.bindings.newTab",
            "notifications.sound",
            "workspaceColors.colors",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing"
            );
        }
        // Sorted and free of duplicates, so `list-supported` output is stable.
        let mut sorted = paths.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted, paths);
    }

    /// A file that only contains a macOS-only section still parses and warns, never errors.
    #[test]
    fn absent_file_validates_as_empty_settings() {
        assert!(validate(&Value::Null).is_empty());
        assert!(validate(&parse_jsonc("{}").unwrap()).is_empty());
    }

    /// A throwaway directory removed when the test ends, so a write test can inspect the file.
    struct Sandbox(std::path::PathBuf);

    impl Sandbox {
        /// A fresh directory named after the test, unique per process and per call.
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "cmux-settings-{label}-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("sandbox directory");
            Self(path)
        }

        /// The directory itself, for a test that needs to pass a working directory.
        fn dir(&self) -> &Path {
            &self.0
        }

        /// A file path inside the sandbox; the file itself is not created.
        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        /// Write `text` to `name`, creating the sandbox, and return the path.
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.file(name);
            std::fs::write(&path, text).expect("seed file");
            path
        }

        /// The exact bytes of `name`, so a test can prove nothing was written.
        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.file(name)).expect("read back")
        }

        /// Every entry left in the sandbox, so a test can spot a leftover temporary file.
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(&self.0)
                .expect("sandbox listing")
                .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Setting a key in a file that does not exist yet creates the file, and only that key.
    #[test]
    fn set_on_a_missing_file_creates_it() {
        let sandbox = Sandbox::new("create");
        let path = sandbox.file("cmux.json");
        assert!(!path.exists());

        let outcome = apply(
            &path,
            "app.appearance",
            Some(&serde_json::json!("dark")),
            Scope::Global,
        )
        .expect("the write succeeds");
        assert_eq!(outcome.status, WriteStatus::Persisted);
        assert_eq!(outcome.key, "app.appearance");

        let value = read(&path).expect("the file is valid JSONC");
        assert_eq!(get(&value, "app.appearance"), Some(&Value::from("dark")));
        // A brand new file holds exactly the key that was asked for.
        assert_eq!(
            value.as_object().map(serde_json::Map::len),
            Some(1),
            "{value}"
        );
    }

    /// A value change rewrites the value only: comments, spacing and key order all survive.
    #[test]
    fn set_preserves_comments_and_layout() {
        let sandbox = Sandbox::new("comments");
        let seeded = r#"{
    // hand written, keep this comment
    "app": {
        "appearance": "system"   // and this one
    },
    "sidebar": { "showPorts": true },
}
"#;
        sandbox.write("cmux.json", seeded);

        apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&serde_json::json!("dark")),
            Scope::Global,
        )
        .expect("the write succeeds");

        let after = sandbox.read("cmux.json");
        assert_eq!(
            after,
            r#"{
    // hand written, keep this comment
    "app": {
        "appearance": "dark"   // and this one
    },
    "sidebar": { "showPorts": true },
}
"#,
            "the comment, the trailing comment and the trailing comma all survive"
        );
        // The replaced value keeps the file's own type: a JSON string, not a bare word.
        assert_eq!(
            get(&read(&sandbox.file("cmux.json")).expect("valid JSONC"), "app.appearance"),
            Some(&Value::from("dark"))
        );
    }

    /// A key that is not in the file yet is added, indented like the members beside it.
    #[test]
    fn set_adds_a_new_nested_key() {
        let sandbox = Sandbox::new("nested");
        sandbox.write(
            "cmux.json",
            "{\n    \"app\": {\"appearance\": \"dark\"},\n    \"sidebar\": {\"showPorts\": true}\n}\n",
        );

        apply(
            &sandbox.file("cmux.json"),
            "sidebar.hideAllDetails",
            Some(&serde_json::json!(true)),
            Scope::Global,
        )
        .expect("the write succeeds");

        let after = sandbox.read("cmux.json");
        assert_eq!(
            after,
            "{\n    \"app\": {\"appearance\": \"dark\"},\n    \"sidebar\": {\"showPorts\": true, \"hideAllDetails\": true}\n}\n",
            "an inline object gains its member inline, without being reflowed"
        );

        // A new section under a multi-line object is indented with the members already there.
        sandbox.write(
            "cmux.json",
            "{\n    \"app\": {\"appearance\": \"dark\"}\n}\n",
        );
        apply(
            &sandbox.file("cmux.json"),
            "markdown.fontSize",
            Some(&serde_json::json!(16)),
            Scope::Global,
        )
        .expect("the write succeeds");
        assert_eq!(
            sandbox.read("cmux.json"),
            "{\n    \"app\": {\"appearance\": \"dark\"},\n    \"markdown\": {\n        \"fontSize\": 16\n    }\n}\n"
        );
    }

    /// A value the schema rejects is refused before anything reaches the disk.
    #[test]
    fn invalid_set_is_refused_without_writing() {
        let sandbox = Sandbox::new("invalid");
        let seeded = "{\n    \"app\": {\"appearance\": \"dark\"}\n}\n";
        sandbox.write("cmux.json", seeded);

        // A word that is not JSON is stored as a string, so the enum is what refuses it.
        let error = apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&Value::from("neon")),
            Scope::Global,
        )
        .expect_err("an unknown appearance is refused");
        let ApplyError::Invalid(issues) = &error else {
            panic!("expected a validation refusal, got {error:?}");
        };
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "app.appearance");
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(sandbox.read("cmux.json"), seeded, "the file is untouched");

        // A wrong type is refused the same way, and still leaves no temporary file behind.
        let error = apply(
            &sandbox.file("cmux.json"),
            "sidebar.showPorts",
            Some(&serde_json::json!(12)),
            Scope::Global,
        )
        .expect_err("a boolean does not take an integer");
        let ApplyError::Invalid(issues) = &error else {
            panic!("expected a validation refusal, got {error:?}");
        };
        assert_eq!(issues[0].message, "expected boolean, found an integer");
        assert_eq!(sandbox.read("cmux.json"), seeded);
        assert_eq!(sandbox.entries(), vec!["cmux.json".to_owned()]);
    }

    /// A value whose text carries comments is refused, not silently stripped of them.
    #[test]
    fn set_refuses_to_drop_comments() {
        let sandbox = Sandbox::new("guarded");
        let seeded = r#"{
    "sidebar": {
        // ports on the left rail
        "showPorts": true
    }
}
"#;
        sandbox.write("cmux.json", seeded);

        // The comment sits inside the value being replaced, so the edit would destroy it.
        let error = apply(
            &sandbox.file("cmux.json"),
            "sidebar",
            Some(&serde_json::json!({"showPorts": false})),
            Scope::Global,
        )
        .expect_err("a commented value is refused");
        assert!(
            matches!(error, ApplyError::Edit(EditError::CommentsInValue(_))),
            "{error:?}"
        );
        assert_eq!(sandbox.read("cmux.json"), seeded, "the file is untouched");

        // Changing the leaf instead is fine: the comment belongs to the line above it.
        apply(
            &sandbox.file("cmux.json"),
            "sidebar.showPorts",
            Some(&serde_json::json!(false)),
            Scope::Global,
        )
        .expect("the leaf carries no comment of its own");
        assert_eq!(
            sandbox.read("cmux.json"),
            r#"{
    "sidebar": {
        // ports on the left rail
        "showPorts": false
    }
}
"#
        );
    }

    /// Removing a member takes its own line with it and leaves its neighbours alone.
    #[test]
    fn unset_removes_the_member_and_its_line() {
        let sandbox = Sandbox::new("unset");
        sandbox.write(
            "cmux.json",
            "{\n    \"app\": {\"appearance\": \"dark\"},\n    \"sidebar\": {\n        \"showPorts\": true,  // ports please\n        \"hideAllDetails\": false\n    }\n}\n",
        );

        let outcome = apply(
            &sandbox.file("cmux.json"),
            "sidebar.showPorts",
            None,
            Scope::Global,
        )
        .expect("the removal succeeds");
        assert_eq!(outcome.status, WriteStatus::Persisted);
        assert_eq!(
            sandbox.read("cmux.json"),
            "{\n    \"app\": {\"appearance\": \"dark\"},\n    \"sidebar\": {\n        \"hideAllDetails\": false\n    }\n}\n",
            "the member, its trailing comment and its own line are gone; the rest is untouched"
        );

        // The result still validates, so the removal did not leave broken JSON behind.
        assert!(validate(&read(&sandbox.file("cmux.json")).expect("valid JSONC"))
            .iter()
            .all(|issue| issue.severity != Severity::Error));
    }

    /// Unsetting a key that is not there is a success that changes nothing.
    #[test]
    fn unset_of_an_absent_key_changes_nothing() {
        let sandbox = Sandbox::new("unchanged");
        let seeded = "{\n    \"app\": {\"appearance\": \"dark\"}\n}\n";
        sandbox.write("cmux.json", seeded);

        let outcome = apply(
            &sandbox.file("cmux.json"),
            "app.accentColor",
            None,
            Scope::Global,
        )
        .expect("an absent key is not an error");
        assert_eq!(outcome.status, WriteStatus::Unchanged);
        assert_eq!(sandbox.read("cmux.json"), seeded, "the file is byte for byte the same");

        // The same holds for a file that does not exist: it is not created by an unset.
        let missing = sandbox.file("absent.json");
        assert_eq!(
            apply(&missing, "app.appearance", None, Scope::Global)
                .expect("an absent file is not an error")
                .status,
            WriteStatus::Unchanged
        );
        assert!(!missing.exists());
    }

    /// A setting that belongs to one scope is an error in the other, and `set` enforces it.
    #[test]
    fn scope_separates_the_global_and_project_files() {
        let value = parse_jsonc(r#"{"terminal": {"scrollSpeed": 2}}"#).unwrap();
        let global = scope_issues(&value, Scope::Global);
        assert!(global.is_empty(), "a terminal setting is a global app setting");
        let project = scope_issues(&value, Scope::Project);
        assert_eq!(project.len(), 1);
        assert_eq!(project[0].path, "terminal");
        assert_eq!(project[0].message, "only valid in the global cmux.json");
        assert_eq!(project[0].severity, Severity::Error);

        // The other direction: a project-only section in the user-wide file.
        let value = parse_jsonc(r#"{"ui": {"newWorkspace": {}}}"#).unwrap();
        assert!(scope_issues(&value, Scope::Project).is_empty());
        let misplaced = scope_issues(&value, Scope::Global);
        assert_eq!(misplaced.len(), 1);
        assert_eq!(misplaced[0].message, "only valid in a project cmux.json");

        // Sections both files honor are honored in both, including a single path inside a
        // section that is otherwise global.
        let shared = parse_jsonc(
            r#"{"actions": {"dev": {"type": "workspaceCommand", "name": "dev"}}, "notifications": {"hooks": []}}"#,
        )
        .unwrap();
        assert!(scope_issues(&shared, Scope::Project).is_empty());
        assert!(scope_issues(&shared, Scope::Global).is_empty());

        // Everything else in `notifications` stays global even when the hooks are project-local.
        let mixed = parse_jsonc(r#"{"notifications": {"hooks": [], "sound": "Ping"}}"#).unwrap();
        let issues = scope_issues(&mixed, Scope::Project);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].path, "notifications.sound");
    }

    /// `set` refuses a setting that does not belong in the file's scope, writing nothing.
    #[test]
    fn set_respects_the_file_scope() {
        let sandbox = Sandbox::new("scope");
        let seeded = "{\n    \"actions\": {\"dev\": {\"type\": \"workspaceCommand\", \"name\": \"dev\"}}\n}\n";
        sandbox.write("cmux.json", seeded);

        let error = apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&serde_json::json!("dark")),
            Scope::Project,
        )
        .expect_err("a global setting does not belong in a project file");
        let ApplyError::Invalid(issues) = &error else {
            panic!("expected a scope refusal, got {error:?}");
        };
        assert_eq!(issues[0].path, "app");
        assert_eq!(issues[0].message, "only valid in the global cmux.json");
        assert_eq!(sandbox.read("cmux.json"), seeded);

        // The same write into the global file is what a user actually wants.
        apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&serde_json::json!("dark")),
            Scope::Global,
        )
        .expect("the global file accepts a global setting");
        assert_eq!(
            get(&read(&sandbox.file("cmux.json")).expect("valid JSONC"), "app.appearance"),
            Some(&Value::from("dark"))
        );
    }

    /// The scope is read off the file's path: the global file, or a `cmux.json` at or above the
    /// working directory. Anything else is global, which is where `cmux config` points.
    #[test]
    fn scope_is_inferred_from_the_file_path() {
        let sandbox = Sandbox::new("infer");
        let here = sandbox.dir();

        assert_eq!(infer_scope_in(&here.join("cmux.json"), here), Scope::Project);
        assert_eq!(
            infer_scope_in(&here.join(".cmux/cmux.json"), here),
            Scope::Project
        );
        // A parent of the working directory is a project file too.
        assert_eq!(infer_scope_in(&here.join("cmux.json"), &here.join("child")), Scope::Project);
        // A child directory, a differently named file and a missing name are all global.
        assert_eq!(
            infer_scope_in(&here.join("child/cmux.json"), here),
            Scope::Global
        );
        assert_eq!(infer_scope_in(&here.join("cmux.jsonc"), here), Scope::Global);
        assert_eq!(infer_scope_in(&here.join("settings.json"), here), Scope::Global);
        // The real global path is global whatever the working directory is.
        if let Some(global) = global_path() {
            assert_eq!(infer_scope_in(&global, here), Scope::Global);
        }
    }

    /// An error the file already had does not block a change that does not add one, so a key
    /// from a newer cmux does not make the file unwritable.
    #[test]
    fn pre_existing_errors_do_not_block_an_unrelated_change() {
        let sandbox = Sandbox::new("preexisting");
        let seeded = "{\n    \"app\": {\"appearanc\": \"dark\"}\n}\n";
        sandbox.write("cmux.json", seeded);
        assert_eq!(Issue::errors(&validate(&parse_jsonc(seeded).unwrap())), 1);

        let outcome = apply(
            &sandbox.file("cmux.json"),
            "markdown.fontSize",
            Some(&serde_json::json!(16)),
            Scope::Global,
        )
        .expect("an unknown key from a newer cmux is not this change's problem");
        assert_eq!(outcome.status, WriteStatus::Persisted);
        let after = read(&sandbox.file("cmux.json")).expect("valid JSONC");
        assert_eq!(
            get(&after, "markdown.fontSize"),
            Some(&Value::from(16))
        );
        // The pre-existing error is still reported, not quietly swallowed.
        assert!(scope_issues(&after, Scope::Global).is_empty());
    }

    /// A path that cannot be reached, or that is not a path at all, is refused before any write.
    #[test]
    fn unreachable_and_malformed_paths_are_refused() {
        let sandbox = Sandbox::new("refuse");
        let seeded = "{\n    \"app\": \"dark\"\n}\n";
        sandbox.write("cmux.json", seeded);

        // `app` is a string, so there is no object to hang `appearance` from.
        let error = apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&serde_json::json!("dark")),
            Scope::Global,
        )
        .expect_err("a scalar cannot hold a nested key");
        assert!(matches!(error, ApplyError::Edit(EditError::NotAnObject(_))), "{error:?}");

        for key in ["", "app..appearance", ".app", "app."] {
            let error = apply(
                &sandbox.file("cmux.json"),
                key,
                Some(&serde_json::json!("dark")),
                Scope::Global,
            )
            .expect_err(&format!("'{key}' is not a settings path"));
            assert!(matches!(error, ApplyError::Edit(EditError::NotAnObject(_))), "{key}");
        }
        assert_eq!(sandbox.read("cmux.json"), seeded);
    }

    /// The write is a temporary file plus one rename, so a reader never sees a partial file
    /// and no scratch file is left behind.
    #[test]
    fn write_is_atomic_and_leaves_no_temporary() {
        let sandbox = Sandbox::new("atomic");
        sandbox.write("cmux.json", "{\n    \"app\": {\"appearance\": \"dark\"}\n}\n");
        apply(
            &sandbox.file("cmux.json"),
            "app.appearance",
            Some(&serde_json::json!("light")),
            Scope::Global,
        )
        .expect("the write succeeds");
        assert_eq!(sandbox.entries(), vec!["cmux.json".to_owned()]);
        assert_eq!(sandbox.read("cmux.json"), "{\n    \"app\": {\"appearance\": \"light\"}\n}\n");
    }

    /// `cmux config docs` names the files, the format, the scopes and real keys.
    #[test]
    fn docs_name_the_files_the_scopes_and_some_keys() {
        let text = docs_text();
        assert!(text.contains("cmux.json settings"));
        assert!(text.contains("JSONC"));
        assert!(text.contains(SCHEMA_URL));
        assert!(text.contains(DOCS_URL));
        for key in [
            "app.appearance",
            "sidebar.showPorts",
            "shortcuts.bindings.<actionId>",
            "cmux config set",
            "cmux config unset",
        ] {
            assert!(text.contains(key), "{key} missing from the docs:\n{text}");
        }

        let payload = docs_json();
        assert_eq!(payload["format"], "jsonc");
        assert_eq!(payload["schema_url"], SCHEMA_URL);
        let sections = &payload["sections"];
        assert!(sections["project"]
            .as_array()
            .expect("project sections")
            .iter()
            .any(|name| name == "ui"));
        assert!(sections["shared"]
            .as_array()
            .expect("shared sections")
            .iter()
            .any(|name| name == "actions"));
        assert!(sections["global"]
            .as_array()
            .expect("global sections")
            .iter()
            .any(|name| name == "sidebar"));
        assert_eq!(payload["known_paths"], known_paths().len());
    }
}
