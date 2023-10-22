use std::collections::HashSet;
use std::fmt::Display;

use enumset::{EnumSet, EnumSetType};
use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub errors: Vec<String>,
}

#[derive(EnumSetType, Debug)]
pub enum Options {
    IgnoreMissingTags,
    IgnoreExternalReferences,
    IgnoreNonUniqOperationIDs,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{} errors found:", self.errors.len())?;
        for error in &self.errors {
            writeln!(f, "- {}", error)?;
        }
        Ok(())
    }
}

pub trait Validate {
    fn validate(&self, _options: EnumSet<Options>) -> Result<(), Error> {
        Ok(())
    }
}

pub trait ValidateWithContext<T> {
    fn validate_with_context(&self, ctx: &mut Context<T>, path: String);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Context<'a, T> {
    pub spec: &'a T,
    pub visited: HashSet<String>,
    pub errors: Vec<String>,
    pub options: EnumSet<Options>,
}

impl Context<'_, ()> {
    pub fn new<T>(spec: &T, options: EnumSet<Options>) -> Context<T> {
        Context {
            spec,
            visited: HashSet::new(),
            errors: Vec::new(),
            options,
        }
    }
}

pub fn validate_email<T>(email: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(email) = email {
        if !email.contains('@') {
            ctx.errors.push(format!(
                "{}: must be a valid email address, found `{}`",
                path, email
            ));
        }
    }
}

const HTTP: &str = "http://";
const HTTPS: &str = "https://";

pub fn validate_url<T>(url: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(url) = url {
        if !url.starts_with(HTTP) && !url.starts_with(HTTPS) {
            ctx.errors
                .push(format!("{}: must be a valid URL, found `{}`", path, url));
        }
    }
}

pub fn validate_required_string<T>(s: &str, ctx: &mut Context<T>, path: String) {
    if s.is_empty() {
        ctx.errors.push(format!("{}: must not be empty", path));
    }
}

pub fn validate_string_matches<T>(s: &str, pattern: &Regex, ctx: &mut Context<T>, path: String) {
    if !pattern.is_match(s) {
        ctx.errors.push(format!(
            "{}: must match pattern `{}`, found `{}`",
            path, pattern, s
        ));
    }
}

pub fn validate_optional_string_matches<T>(
    s: &Option<String>,
    pattern: &Regex,
    ctx: &mut Context<T>,
    path: String,
) {
    if let Some(s) = s {
        validate_string_matches(s, pattern, ctx, path);
    }
}
