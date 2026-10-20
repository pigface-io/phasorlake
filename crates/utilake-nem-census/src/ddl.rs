//! AEMO's create script, statement by statement.
//!
//! AEMO generates the scripts from one model with PowerDesigner, so they are
//! regular: `create table`, `comment on table`, `comment on column`,
//! `alter table ... add constraint ... primary key | unique`, and
//! `create index`. Anything else is an error, so a new kind of statement in
//! a later release is noticed rather than skipped. The census keeps the
//! tables, their columns (name, type as written, nullability), their
//! primary keys and the comments; unique constraints and indexes are read
//! and dropped.

use std::collections::BTreeMap;

/// One script, read.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Script {
    /// `AEMO Electricity Data Model v5.7.0.0 postgreSQL`, from the header.
    pub model: String,
    /// `2026/03/06 11:11:46`, from the header.
    pub created: String,
    /// Tables by name.
    pub tables: BTreeMap<String, ScriptTable>,
}

/// A table as the script declares it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScriptTable {
    /// Columns in declared order.
    pub columns: Vec<ScriptColumn>,
    /// `comment on table`.
    pub comment: Option<String>,
    /// `primary key (...)`, in key order.
    pub primary_key: Vec<String>,
}

/// A column as the script declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptColumn {
    /// Name as written.
    pub name: String,
    /// Type as written, upper case: `NUMERIC(15,5)`, `VARCHAR(10)`, `TIMESTAMP(0)`.
    pub ty: String,
    /// Whether nulls are allowed.
    pub nullable: bool,
    /// `comment on column`.
    pub comment: Option<String>,
}

/// Why a statement could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("statement {index} ({sample}): {reason}")]
pub struct DdlError {
    /// 1-based position in the script.
    pub index: usize,
    /// What is wrong.
    pub reason: String,
    /// The statement's first 60 characters.
    pub sample: String,
}

/// Reads a whole script.
pub fn parse(text: &str) -> Result<Script, DdlError> {
    let mut script = Script::default();
    let (header, body) = split_comments(text);
    for line in header.lines() {
        if let Some(v) = line.trim().strip_prefix("Data Model:") {
            script.model = v.trim().to_owned();
        } else if let Some(v) = line.trim().strip_prefix("Created on:") {
            script.created = v.trim().to_owned();
        }
    }
    for (i, stmt) in split_statements(&body).into_iter().enumerate() {
        statement(&mut script, &stmt).map_err(|reason| DdlError {
            index: i + 1,
            reason,
            sample: stmt.chars().take(60).collect::<String>().replace('\n', " "),
        })?;
    }
    Ok(script)
}

/// Takes `/* ... */` blocks out of the text, returning their contents (the
/// header) and the text without them. Quoted strings are left alone.
fn split_comments(text: &str) -> (String, String) {
    let mut comments = String::new();
    let mut body = String::with_capacity(text.len());
    let mut rest = text;
    let mut in_quote = false;
    while let Some(c) = rest.chars().next() {
        if in_quote {
            body.push(c);
            if c == '\'' {
                in_quote = false;
            }
            rest = &rest[c.len_utf8()..];
        } else if c == '\'' {
            in_quote = true;
            body.push(c);
            rest = &rest[1..];
        } else if let Some(after) = rest.strip_prefix("/*") {
            let end = after.find("*/").unwrap_or(after.len());
            comments.push_str(&after[..end]);
            comments.push('\n');
            rest = after.get(end + 2..).unwrap_or("");
        } else {
            body.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    (comments, body)
}

/// Splits on `;` outside quotes; blank statements are dropped.
fn split_statements(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in body.chars() {
        match c {
            '\'' => {
                in_quote = !in_quote;
                cur.push(c);
            }
            ';' if !in_quote => {
                if !cur.trim().is_empty() {
                    out.push(cur.trim().to_owned());
                }
                cur.clear();
            }
            '\r' => {}
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

/// One statement. Keywords are matched on a lower-cased copy; identifiers
/// and quoted text are taken from the original at the same offsets.
fn statement(script: &mut Script, stmt: &str) -> Result<(), String> {
    let lower = stmt.to_ascii_lowercase();
    if let Some(at) = after_prefix(&lower, "create table ") {
        let name = ident(stmt, at).ok_or("no table name")?;
        let (inner, tail) = parens(&stmt[at + name.len()..])?;
        let mut table = ScriptTable::default();
        for part in split_top(inner) {
            table.columns.push(column(part)?);
        }
        tablespace(tail)?;
        if script.tables.insert(name.to_owned(), table).is_some() {
            return Err(format!("table {name} is declared twice"));
        }
    } else if let Some(at) = after_prefix(&lower, "comment on table ") {
        let name = ident(stmt, at).ok_or("no table name")?;
        let text = quoted(stmt, &lower, at + name.len())?;
        table_mut(script, name)?.comment = Some(text);
    } else if let Some(at) = after_prefix(&lower, "comment on column ") {
        let path = ident(stmt, at).ok_or("no column name")?;
        let (name, col) = path.split_once('.').ok_or("column is not table.column")?;
        let text = quoted(stmt, &lower, at + path.len())?;
        let table = table_mut(script, name)?;
        let column = table
            .columns
            .iter_mut()
            .find(|c| c.name == col)
            .ok_or_else(|| format!("table {name} has no column {col}"))?;
        column.comment = Some(text);
    } else if let Some(at) = after_prefix(&lower, "alter table ") {
        let name = ident(stmt, at).ok_or("no table name")?;
        let at = skip_space(&lower, at + name.len());
        let at = after_prefix(&lower[at..], "add constraint ")
            .map(|n| at + n)
            .ok_or("alter table is not add constraint")?;
        let cname = ident(stmt, at).ok_or("no constraint name")?;
        let at = skip_space(&lower, at + cname.len());
        if let Some(n) = after_prefix(&lower[at..], "primary key") {
            let (cols, tail) = parens(&stmt[at + n..])?;
            let table = table_mut(script, name)?;
            if !table.primary_key.is_empty() {
                return Err(format!("table {name} has two primary keys"));
            }
            table.primary_key = names(cols);
            using_index(tail)?;
        } else if let Some(n) = after_prefix(&lower[at..], "unique") {
            let (_, tail) = parens(&stmt[at + n..])?;
            table_mut(script, name)?;
            using_index(tail)?;
        } else {
            return Err("constraint is neither primary key nor unique".to_owned());
        }
    } else if let Some(at) = after_prefix(&lower, "create") {
        let at = skip_space(&lower, at);
        let at = match after_prefix(&lower[at..], "unique ") {
            Some(n) => skip_space(&lower, at + n),
            None => at,
        };
        let at = after_prefix(&lower[at..], "index ")
            .map(|n| at + n)
            .ok_or("create of something other than a table or index")?;
        let iname = ident(stmt, at).ok_or("no index name")?;
        let at = skip_space(&lower, at + iname.len());
        let at = after_prefix(&lower[at..], "on ")
            .map(|n| at + n)
            .ok_or("index without on")?;
        let name = ident(stmt, at).ok_or("no table name")?;
        let (_, tail) = parens(&stmt[at + name.len()..])?;
        tablespace(tail)?;
        table_mut(script, name)?;
    } else {
        return Err("not a statement the model uses".to_owned());
    }
    Ok(())
}

/// The offset just past `prefix`, when `s` starts with it.
fn after_prefix(s: &str, prefix: &str) -> Option<usize> {
    s.starts_with(prefix).then_some(prefix.len())
}

/// The offset of the first non-space at or after `at`.
fn skip_space(s: &str, at: usize) -> usize {
    at + s[at..].len() - s[at..].trim_start().len()
}

fn table_mut<'a>(script: &'a mut Script, name: &str) -> Result<&'a mut ScriptTable, String> {
    script
        .tables
        .get_mut(name)
        .ok_or_else(|| format!("table {name} is not declared"))
}

/// The identifier at `at` in `stmt`, as written.
fn ident(stmt: &str, at: usize) -> Option<&str> {
    let rest = &stmt[at..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// The original-case text between the first `(` and its matching `)` of
/// `s`, and what follows the `)`.
fn parens(s: &str) -> Result<(&str, &str), String> {
    let start = skip_space(s, 0);
    if !s[start..].starts_with('(') {
        return Err("missing (".to_owned());
    }
    let mut depth = 0usize;
    let mut in_quote = false;
    for (i, c) in s[start..].char_indices() {
        match c {
            '\'' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&s[start + 1..start + i], &s[start + i + 1..]));
                }
            }
            _ => {}
        }
    }
    Err("unbalanced (".to_owned())
}

/// Splits on commas outside parentheses and quotes.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut in_quote, mut last) = (0usize, false, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => depth -= 1,
            ',' if !in_quote && depth == 0 => {
                out.push(s[last..i].trim());
                last = i + 1;
            }
            _ => {}
        }
    }
    let tail = s[last..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn names(cols: &str) -> Vec<String> {
    split_top(cols)
        .into_iter()
        .map(|c| c.strip_suffix(" ASC").unwrap_or(c).to_owned())
        .collect()
}

/// `NAME TYPE [not null | null] [default X]`.
fn column(part: &str) -> Result<ScriptColumn, String> {
    let (name, rest) = part
        .split_once(char::is_whitespace)
        .ok_or_else(|| format!("column `{part}` has no type"))?;
    let rest = rest.trim_start();
    let type_end = match rest.find('(') {
        Some(p) => {
            let close = rest[p..].find(')').ok_or("type with unbalanced (")?;
            p + close + 1
        }
        None => rest.find(char::is_whitespace).unwrap_or(rest.len()),
    };
    let ty: String = rest[..type_end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    // What follows the type: a nullability, a default, or both in either
    // order (postgreSQL writes `not null default X`, Oracle `default X not null`).
    let mut nullable = true;
    let mut tail = rest[type_end..].trim().to_owned();
    if let Some(p) = tail.to_ascii_lowercase().find("default ") {
        let value = tail[p + 8..].trim().to_ascii_lowercase();
        if value.ends_with("not null") {
            nullable = false;
        }
        tail.truncate(p);
    }
    match tail.trim().to_ascii_lowercase().as_str() {
        "" | "null" => {}
        "not null" => nullable = false,
        other => return Err(format!("column {name}: `{other}` is not a nullability")),
    }
    Ok(ScriptColumn {
        name: name.to_owned(),
        ty,
        nullable,
        comment: None,
    })
}

/// The quoted text of a comment statement, `is '...'` from offset `at`;
/// `''` is one quote.
fn quoted(stmt: &str, lower: &str, at: usize) -> Result<String, String> {
    let at = skip_space(lower, at);
    let at = after_prefix(&lower[at..], "is")
        .map(|n| at + n)
        .ok_or("comment without is")?;
    let text = stmt[at..].trim();
    let inner = text
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .ok_or("comment text is not quoted")?;
    Ok(inner.replace("''", "'"))
}

/// Oracle's `tablespace X` after a table or index; nothing in postgreSQL.
fn tablespace(tail: &str) -> Result<(), String> {
    let t = tail.trim();
    if t.is_empty() || t.to_ascii_lowercase().starts_with("tablespace ") {
        Ok(())
    } else {
        Err(format!("unexpected `{t}` after the column list"))
    }
}

/// Oracle's `using index tablespace X` after a constraint; nothing in postgreSQL.
fn using_index(tail: &str) -> Result<(), String> {
    let t = tail.trim().to_ascii_lowercase();
    if t.is_empty() || t.starts_with("using index tablespace ") {
        Ok(())
    } else {
        Err(format!("unexpected `{t}` after the constraint"))
    }
}

/// Windows-1252, the encoding the scripts are written in: ASCII but for a
/// few non-breaking spaces and typographic quotes in comments. Every byte
/// decodes; nothing fails.
pub fn decode_cp1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: &str = "/*==*/\r\n/* Data Model:     AEMO Electricity Data Model v5.7.0.0 postgreSQL*/\r\n/* Created on:     2026/03/06 11:11:46 */\r\n\
create table T (\r\n   A                    VARCHAR(10)          not null,\r\n   B                    NUMERIC(15,5)        null,\r\n   C TIMESTAMP(0) not null default 'X; /* y */',\r\n   D NUMERIC(3) null,\r\n   E NUMERIC(1) default 1  not null,\r\n   F VARCHAR(1) default 'A' null\r\n);\r\n\
comment on table T is\r\n'Table ''T''; with a semicolon';\r\n\
comment on column T.A is\r\n'Col A\nsecond line';\r\n\
alter table T\r\n   add constraint T_PK primary key (A, B);\r\n\
alter table T add constraint T_UK unique (C);\r\n\
create  index T_LCX on T (\r\nC\r\n);\r\ncreate unique index T_UX on T (A, D);\r\n";

    const ORA: &str = "/* Data Model: AEMO Electricity Data Model v5.7.0.0 Oracle*/\n\
create table T (\n   A VARCHAR2(10) not null,\n   B NUMBER(15,5)\n)\ntablespace PSSTAB01;\n\
alter table T\n   add constraint T_PK primary key (A ASC)\n      using index tablespace PSSIND01;\n\
alter table T add constraint T_UK unique (B) using index tablespace PSSIND01;\n\
create index T_LCX on T (\n   B ASC\n)\ntablespace PSSIND01;\n";

    #[test]
    fn reads_the_postgres_shape() {
        let s = parse(PG).unwrap();
        assert_eq!(s.model, "AEMO Electricity Data Model v5.7.0.0 postgreSQL");
        assert_eq!(s.created, "2026/03/06 11:11:46");
        let t = &s.tables["T"];
        let names: Vec<_> = t.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C", "D", "E", "F"]);
        assert!(!t.columns[0].nullable);
        assert!(t.columns[1].nullable);
        assert_eq!(t.columns[1].ty, "NUMERIC(15,5)");
        assert_eq!(t.columns[2].ty, "TIMESTAMP(0)");
        assert!(!t.columns[2].nullable);
        assert_eq!(t.columns[3].ty, "NUMERIC(3)");
        assert!(!t.columns[4].nullable);
        assert!(t.columns[5].nullable);
        assert_eq!(t.comment.as_deref(), Some("Table 'T'; with a semicolon"));
        assert_eq!(t.columns[0].comment.as_deref(), Some("Col A\nsecond line"));
        assert_eq!(t.primary_key, ["A", "B"]);
    }

    #[test]
    fn reads_the_oracle_clauses() {
        let s = parse(ORA).unwrap();
        let t = &s.tables["T"];
        assert_eq!(t.primary_key, ["A"]);
        assert_eq!(t.columns[0].ty, "VARCHAR2(10)");
        assert!(t.columns[1].nullable);
    }

    #[test]
    fn every_unknown_shape_is_an_error() {
        let cases = [
            ("drop table T;", "not a statement"),
            (
                "create table T (A VARCHAR(1) null); create table T (A VARCHAR(1) null);",
                "twice",
            ),
            ("create table (A VARCHAR(1));", "no table name"),
            ("create table T A VARCHAR(1);", "missing ("),
            ("create table T (A VARCHAR(1);", "unbalanced"),
            ("create table T (A);", "has no type"),
            ("create table T (A VARCHAR(1 null);", "unbalanced ("),
            ("create table T (A VARCHAR(1) maybe);", "not a nullability"),
            (
                "create table T (A VARCHAR(1)) organization heap;",
                "unexpected",
            ),
            ("comment on table T is 'x';", "not declared"),
            (
                "create table T (A VARCHAR(1)); comment on table T 'x';",
                "without is",
            ),
            (
                "create table T (A VARCHAR(1)); comment on table T is x;",
                "not quoted",
            ),
            (
                "create table T (A VARCHAR(1)); comment on column T is 'x';",
                "not table.column",
            ),
            (
                "create table T (A VARCHAR(1)); comment on column T.B is 'x';",
                "no column B",
            ),
            (
                "create table T (A VARCHAR(1)); comment on column (;",
                "no column name",
            ),
            (
                "create table T (A VARCHAR(1)); comment on table (;",
                "no table name",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T drop column A;",
                "not add constraint",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint X check (A > 0);",
                "neither",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint X primary key (A) enable;",
                "unexpected",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint X primary key (A); alter table T add constraint Y primary key (A);",
                "two primary keys",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint primary key (A);",
                "neither",
            ),
            (
                "alter table  add constraint X primary key (A);",
                "no table name",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint (;",
                "no constraint name",
            ),
            (
                "create table T (A VARCHAR(1)); alter table T add constraint X unique (A) enable;",
                "unexpected",
            ),
            ("create view V as select 1;", "other than a table or index"),
            ("create index on T (A);", "index without on"),
            ("create index  on T (A);", "no index name"),
            ("create index I T (A);", "index without on"),
            ("create index I on (A);", "no table name"),
            ("create index I on T (A);", "not declared"),
            (
                "create table T (A VARCHAR(1)); create index I on T (A) nologging;",
                "unexpected",
            ),
        ];
        for (ddl, expect) in cases {
            let err = parse(ddl).unwrap_err().to_string();
            assert!(err.contains(expect), "{ddl:?} gave {err:?}");
        }
    }

    #[test]
    fn comments_quotes_and_bytes() {
        let (header, body) = split_comments("/* a */x'/* not */'y/* b");
        assert_eq!(header, " a \n b\n");
        assert_eq!(body, "x'/* not */'y");
        assert_eq!(split_statements("a;'b;c';d; ;"), ["a", "'b;c'", "d"]);
        assert_eq!(split_statements("a;b"), ["a", "b"]);
        assert_eq!(
            split_top("A, B(1,2), 'x,y', C"),
            ["A", "B(1,2)", "'x,y'", "C"]
        );
        assert_eq!(parse("").unwrap(), Script::default());
        assert_eq!(
            decode_cp1252(b"a\xA0b\x80\x91\x81\xE9"),
            "a\u{A0}b\u{20AC}\u{2018}\u{FFFD}\u{E9}"
        );
    }
}
