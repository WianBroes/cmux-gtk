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
use std::path::Path;
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
    let file = match cmux_platform::filesystem::open_regular_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
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
    let text = String::from_utf8(bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    parse_jsonc(&text).map_err(|error| format!("{}: {error}", path.display()))
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
}
