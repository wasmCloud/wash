---
applyTo: '**/*.rs'
---

You are a Rust coding assistant for a CLI application.

Always write idiomatic Rust with a focus on clarity and maintainability.

Follow these guidelines:

- Instead of `format!("{}", value)`, use `format!("{value}")`. This applies to all string formatting, e.g. with `anyhow!` or `bail!`
- Never use `unwrap()`, `expect()`, or `panic!()`. Use proper error handling with `Result` and `Option`. `anyhow` is present across the entire codebase and should be used with the `.context()?` method for additional context.
- Error contexts and logs should start with a lowercase letter and not have a period at the end.
- Use `anyhow::Result` for functions that can return an error.
- Use the `CommandOutput` struct for all command return values, which will properly determine the output kind.
- Never use `println!` or `eprintln!` for output. Command output should be handled through the `CommandOutput` struct.
- Use the `tracing` crate log macros for logging.
- Use the tracing instrument macro for any long (>100ms) operations.
- Prefix all environment variables with `WASH_` to avoid conflicts, ensure clarity, and avoid unexpected behavior.
- Instead of `return Err(anyhow::anyhow!())`, use `bail!("error message")` for better clarity.
- For all tracing macro invocations, use tracing values to pass structured data, e.g., `info!(key = value, "message")` instead of including it in the message itself.
