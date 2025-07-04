use enumset::EnumSet;
use regex::Regex;
use std::collections::HashSet;
use std::fmt;

use crate::validation::{Error, Options};

/// ValidateWithContext is a trait for validating an object with a context.
/// It allows the object to be validated with additional context information,
/// such as the specification and validation options.
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
            self.errors.push(format!("{path}{msg}"));
        } else {
            self.errors.push(format!("{path}: {msg}"));
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

/// Validates that the given optional email string contains an '@' character.
/// If the email is present and invalid, records an error in the context.
pub fn validate_email<T>(email: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(email) = email {
        if !email.contains('@') {
            ctx.error(
                path,
                format_args!("must be a valid email address, found `{email}`"),
            );
        }
    }
}

const HTTP: &str = "http://";
const HTTPS: &str = "https://";

/// Validates an optional URL string.
/// If the URL is present, it checks if it is valid using `validate_required_url`.
/// Records an error in the context if the URL is invalid.
pub fn validate_optional_url<T>(url: &Option<String>, ctx: &mut Context<T>, path: String) {
    if let Some(url) = url {
        validate_required_url(url, ctx, path);
    }
}

/// Validates that the given URL string starts with "http://" or "https://".
/// If the URL is invalid, records an error in the context.
pub fn validate_required_url<T>(url: &String, ctx: &mut Context<T>, path: String) {
    if ctx.is_option(Options::IgnoreInvalidUrls) {
        return;
    }

    // TODO: Consider using a more robust URL validation library.
    if !url.starts_with(HTTP) && !url.starts_with(HTTPS) {
        ctx.error(path, format_args!("must be a valid URL, found `{url}`"));
    }
}

/// Validates that the given string is not empty.
/// If the string is empty, records an error in the context.
pub fn validate_required_string<T>(s: &str, ctx: &mut Context<T>, path: String) {
    if s.is_empty() {
        ctx.error(path, "must not be empty");
    }
}

/// Validates that the given string matches the provided regex pattern.
/// If the string does not match, records an error in the context with details.
pub fn validate_string_matches<T>(s: &str, pattern: &Regex, ctx: &mut Context<T>, path: String) {
    if !pattern.is_match(s) {
        ctx.error(
            path,
            format_args!("must match pattern `{pattern}`, found `{s}`"),
        );
    }
}

// Validates an optional string against a regex pattern if present.
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

/// Validates that the given regex pattern is valid.
/// If the pattern is invalid, records an error in the context with details.
pub fn validate_pattern<T>(pattern: &str, ctx: &mut Context<T>, path: String) {
    match Regex::new(pattern) {
        Ok(_) => {}
        Err(e) => ctx.error(path, format_args!("pattern `{pattern}` is invalid: {e}")),
    }
}

/// Validates that the given object has not been visited before,
/// optionally ignoring the check based on the provided option.
/// If the object has already been visited and the ignore option is not set, an error is recorded.
/// Then, the object's own validation logic is invoked.
pub fn validate_not_visited<T, D>(
    obj: &D,
    ctx: &mut Context<T>,
    ignore_option: Options,
    path: String,
) where
    D: ValidateWithContext<T>,
{
    if ctx.visit(path.clone()) {
        if !ctx.is_option(ignore_option) {
            ctx.error(path.clone(), "unused");
        }
        obj.validate_with_context(ctx, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url() {
        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(
            &String::from("http://example.com"),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(
            &String::from("https://example.com"),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_required_url(&String::from("foo-bar"), &mut ctx, String::from("test_url"));
        assert!(
            ctx.errors
                .contains(&"test_url: must be a valid URL, found `foo-bar`".to_string()),
            "expected error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&(), Options::only(&Options::IgnoreInvalidUrls));
        validate_required_url(&String::from("foo-bar"), &mut ctx, String::from("test_url"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(&None, &mut ctx, String::from("test_url"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("http://example.com")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("https://example.com")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        let mut ctx = Context::new(&(), Options::new());
        validate_optional_url(
            &Some(String::from("foo-bar")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(
            ctx.errors
                .contains(&"test_url: must be a valid URL, found `foo-bar`".to_string()),
            "expected error: {:?}",
            ctx.errors
        );

        let mut ctx = Context::new(&(), Options::only(&Options::IgnoreInvalidUrls));
        validate_optional_url(
            &Some(String::from("foo-bar")),
            &mut ctx,
            String::from("test_url"),
        );
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);
    }
}
