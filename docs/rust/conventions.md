# Rust Conventions

Rules for any Rust code in Localify. For architecture (module structure, DDD layering, layer boundary rules), see [architecture.md](architecture.md).

## Philosophy

Pragmatic, idiomatic Rust. Build the simplest thing that works. YAGNI throughout. Three similar lines beats one clever abstraction.

## Rules

### [RS-01] No panics in production paths

Use `Result<T, E>` for error handling. Never use `.unwrap()` or `.expect()` in production code. Use `?` liberally for clean propagation. Panics are acceptable only in tests and in genuinely unreachable branches (and even then, prefer `unreachable!()` with a reason).

### [RS-02] Error handling layering

- `anyhow::Result` for application-level errors (CLI commands, use cases)
- Custom error types with `thiserror` only when callers need to match on variants
- Errors include context: what failed, why, and what the user should do next

```rust
// Bad
return Err(anyhow!("Domain exists"));

// Good
return Err(anyhow!(
    "Failed to register domain '{}': already registered.\n\
     Use 'localify unregister {}' first, or choose a different name.",
    domain, domain
));
```

### [RS-03] YAGNI

- Don't build features that aren't needed yet
- Don't create abstractions until you need them in multiple places
- Don't add configuration options until someone asks for them
- Don't add plugins, hooks, or extensibility mechanisms
- Start simple; refactor when complexity actually demands it

### [RS-04] Minimize dependencies

Each new crate is a maintenance burden. Prefer well-maintained, popular crates. Read the code of small crates before adding them. Don't add a dependency to save 20 lines.

Current core stack: `tokio`, `axum`, `hyper`, `tower`, `rustls`, `rcgen`, `clap`, `serde`, `serde_json`, `anyhow`, `thiserror`. New deps must be justified in the commit message.

### [RS-05] Types over primitives

Wrap primitives in meaningful types with validation at construction. See [architecture.md](architecture.md) for the value-object pattern.

```rust
// Bad
pub fn register(domain: String, port: u16) -> Result<()>

// Good
pub fn register(domain: DomainName, target: Target) -> Result<()>
```

### [RS-06] No dynamic or clever escape hatches

- No macros for things a function can do
- No generics for a single use case
- No trait hierarchies more than 2 levels deep
- No `async` where blocking I/O works fine (config files, cert generation)
- No `unsafe` without an explicit review (use the `unsafe-checker` skill if your checkout has `.rust-skills/`)

### [RS-07] Function size

Keep functions under 50 lines. Extract helpers when nesting reaches 3+ levels. A function should do one thing.

### [RS-08] Doc comments

Public functions have doc comments that explain *why*, not *what*. Include examples for non-obvious behavior. Don't document what the signature already says.

```rust
// Bad — repeats the signature
/// Registers a domain.
pub fn register_domain(domain: &str, target: Target) -> Result<()> { ... }

// Good — explains behavior and failure modes
/// Registers a new domain with either a file path or port target.
///
/// Returns an error if the domain is already registered or if
/// certificate generation fails.
pub fn register_domain(domain: &str, target: Target) -> Result<()> { ... }
```

### [RS-09] Testing strategy

- Unit tests (`#[cfg(test)]` modules) for business logic in the same file
- Integration tests in `tests/` for CLI commands end-to-end
- Use temporary directories for filesystem tests
- Don't test external libraries, trivial accessors, or private implementation details
- Test error cases, not just happy paths

### [RS-10] Security

- Never log or display certificate private keys
- Validate all user inputs (domain names, paths, ports) at the boundary
- File permissions: `0600` for keys, `0644` for certs
- Require explicit confirmation for destructive operations (unregister, uninstall)
- Run the daemon with the minimum required privileges

### [RS-11] Platform-specific code

Use `#[cfg(target_os = "...")]` for platform differences. macOS support is the priority; Linux is added once macOS works.

```rust
#[cfg(target_os = "macos")]
fn setup_dns() -> Result<()> { /* /etc/resolver/ */ }

#[cfg(target_os = "linux")]
fn setup_dns() -> Result<()> { /* dnsmasq */ }
```

### [RS-12] Conventional commits

`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, etc. Commit messages explain *why*, not *what*.

## Performance targets

Don't optimize until these are violated:

- Daemon startup: < 100ms
- Domain registration: < 500ms (including cert generation)
- Request latency overhead: < 10ms
- Memory idle: < 50MB / under load: < 200MB

## Pre-commit checklist

- [ ] `cargo fmt`
- [ ] `cargo clippy -- -D warnings`
- [ ] `cargo test`
- [ ] No `.unwrap()` / `.expect()` in production paths
- [ ] New deps justified (popular, well-maintained, worth the weight)
- [ ] Error messages are actionable (what failed, why, what to do next)
- [ ] Public functions have doc comments explaining *why*
- [ ] DDD layer rules respected (see [architecture.md](architecture.md))
- [ ] No `unsafe` added without review
