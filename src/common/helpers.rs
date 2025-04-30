use std::collections::HashSet;
use std::fmt;

use enumset::EnumSet;
use regex::Regex;

use crate::validation::{Error, Options};

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

pub trait PushError<T> {
    fn error(&mut self, path: String, args: T);
}

impl<T> PushError<&str> for Context<'_, T> {
    fn error(&mut self, path: String, msg: &str) {
        if msg.starts_with('.') {
            self.errors.push(format!("{}{}", path, msg));
        } else {
            self.errors.push(format!("{}: {}", path, msg));
        }
    }
}

impl<T> PushError<String> for Context<'_, T> {
    fn error(&mut self, path: String, msg: String) {
        self.error(path, msg.as_str());
    }
}

impl<T> PushError<fmt::Arguments<'_>> for Context<'_, T> {
    fn error(&mut self, path: String, args: fmt::Arguments<'_>) {
        self.error(path, args.to_string().as_str());
    }
}

impl<T> Context<'_, T> {
    pub fn reset(&mut self) {
        self.visited.clear();
        self.errors.clear();
    }

    pub fn visit(&mut self, path: String) -> bool {
        self.visited.insert(path)
    }

    pub fn is_visited(&self, path: &str) -> bool {
        self.visited.contains(path)
    }

    pub fn is_option(&self, option: Options) -> bool {
        self.options.contains(option)
    }
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

impl<'a, T> From<Context<'a, T>> for Result<(), Error> {
    fn from(val: Context<'a, T>) -> Self {
        if val.errors.is_empty() {
            Ok(())
        } else {
            Err(Error { errors: val.errors })
        }
    }
}

pub fn validate_email<T>(email: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(email) = email {
        if !email.contains('@') {
            ctx.error(
                path,
                format_args!("must be a valid email address, found `{}`", email),
            );
        }
    }
}

const HTTP: &str = "http://";
const HTTPS: &str = "https://";

pub fn validate_optional_url<T>(url: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(url) = url {
        validate_required_url(url, ctx, path);
    }
}

pub fn validate_required_url<T>(url: &String, ctx: &mut Context<T>, path: String) {
    if !url.starts_with(HTTP) && !url.starts_with(HTTPS) {
        ctx.error(path, format_args!("must be a valid URL, found `{}`", url));
    }
}

pub fn validate_required_string<T>(s: &str, ctx: &mut Context<T>, path: String) {
    if s.is_empty() {
        ctx.error(path, "must not be empty");
    }
}

pub fn validate_string_matches<T>(s: &str, pattern: &Regex, ctx: &mut Context<T>, path: String) {
    if !pattern.is_match(s) {
        ctx.error(
            path,
            format_args!("must match pattern `{}`, found `{}`", pattern, s),
        );
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

pub fn validate_pattern<T>(pattern: &str, ctx: &mut Context<T>, path: String) {
    match Regex::new(pattern) {
        Ok(_) => {}
        Err(e) => ctx.error(
            path,
            format_args!("pattern `{}` is invalid: {}", pattern, e),
        ),
    }
}
