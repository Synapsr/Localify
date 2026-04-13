# Rust Architecture — Roxy

Module structure, DDD layering, and layer boundary rules for the Rust side of Roxy. For general conventions (YAGNI, error handling, testing, style), see [conventions.md](conventions.md).

## Module structure

```
src/
├── main.rs                    # CLI entry point
├── lib.rs                     # Library root
├── cli/                       # CLI layer — thin, delegates to use cases
│   └── commands/
├── domain/                    # Domain layer — core business logic, no I/O
│   ├── entities/              # Things with identity and lifecycle
│   ├── value_objects/         # Validated primitives (DomainName, Port, Target)
│   └── services/              # Operations spanning multiple entities
├── application/               # Use cases + ports
│   ├── ports/                 # Trait interfaces infrastructure implements
│   └── <use_case>.rs
├── infrastructure/            # I/O, persistence, system calls
│   ├── config_store.rs
│   └── system/                # OS-specific (macos.rs, linux.rs)
├── desktop/                   # Tauri-facing DTOs and DesktopApi facade
└── daemon/                    # HTTP server, proxy, TLS, DNS — infrastructure
```

## DDD quick reference

| Pattern | When to use |
|---------|-------------|
| **Value Object** | Primitives with validation rules (`DomainName`, `Port`, `Target`). Immutable, validated in the constructor, compared by value. |
| **Entity** | Things with identity and lifecycle (`DomainRegistration`). Mutable state protected by methods that enforce invariants. |
| **Domain Service** | Operations that don't naturally belong on a single entity (`CertificateService`, `DnsService`). |
| **Use Case (Application)** | Orchestrates domain services for a specific workflow. CLI commands are thin wrappers around use cases. |
| **Repository / Store** | Prefer a concrete `ConfigStore`. Introduce a trait only if you need multiple implementations. |

## Anti-patterns

- Don't create layers just to have layers — validation belongs in the constructor, not in a separate `Validator` trait
- Don't create repositories for value objects or single-implementation storage
- Don't add events, sagas, or complex aggregate rules unless the domain actually needs them
- Don't separate domain and persistence models unless you have a concrete reason — Rust's type system often makes DTOs unnecessary

## Layer Boundary Rules (enforced)

These are non-negotiable. Any violation blocks a merge.

### Rule 1 — Domain does no I/O

`src/domain/` depends only on `std`, `thiserror`, and `anyhow`. No filesystem calls (`path.exists()`, `path.is_dir()`), no network, no `bollard`, no `serde`. Domain types validate **structural invariants only**. Filesystem or network validation lives in infrastructure or application.

```rust
// BAD — domain touches the filesystem
impl DomainRegistration {
    pub fn validate(&self) -> Result<()> {
        if !path.exists() { /* ... */ }
    }
}

// GOOD — structural invariants only
impl DomainRegistration {
    pub fn validate(&self) -> Result<()> {
        if self.routes.is_empty() {
            return Err(RegistrationError::NoRoutes);
        }
        Ok(())
    }
}
```

### Rule 2 — Domain enums don't name infrastructure

Domain enums model what the business cares about, not where data comes from. If no business rule behaves differently per source, use a generic variant.

```rust
// BAD — domain knows about Docker
pub enum RegistrationSource { Config, Docker }

// GOOD
pub enum RegistrationSource { Config, External }
```

Adding a new provider (Podman, Kubernetes) should not require touching `src/domain/`. The provider name is metadata carried by the infrastructure layer.

### Rule 3 — Value objects don't have infrastructure-specific constructors

Type-driven design encodes **domain** distinctions, not infrastructure ones.

```rust
// BAD — domain knows about containers
impl Port {
    pub fn new(port: u16) -> Result<Self> { /* rejects < 1024 */ }
    pub fn container(port: u16) -> Result<Self> { /* allows any */ }
}

// GOOD — one constructor, or model a real domain distinction
impl Port {
    pub fn new(port: u16) -> Result<Self> { /* 1..=65535 */ }
}
```

Ask: "Does any business rule behave differently here?" If yes, model the domain distinction. If no, one constructor is enough.

### Rule 4 — Application ports return domain types or DTOs

Traits in `src/application/ports/` must never expose infrastructure types (`DaemonConfig`, `RoxyPaths`, raw PIDs, OS structures). Define an application-layer DTO if needed.

```rust
// BAD — infrastructure types in port interface
fn load(&self) -> Result<(DaemonConfig, RoxyPaths)>;

// GOOD — application-level DTO
pub struct AppConfig { pub registrations: Vec<DomainRegistration> }
fn load(&self) -> Result<AppConfig>;
```

### Rule 5 — Entities are fully constructed, not mutated by infrastructure

If a property needs to be set (like `source`), pass it via the constructor. Don't add setters for infrastructure to poke values into domain entities.

```rust
// BAD
let mut reg = DomainRegistration::new(pattern, routes);
reg.set_source(RegistrationSource::Docker);

// GOOD
let reg = DomainRegistration::with_source(pattern, routes, RegistrationSource::External);
```

### Rule 6 — `daemon/` is infrastructure

`src/daemon/` holds HTTP server, proxy, TLS, DNS server — all infrastructure. It must not contain application-level orchestration. Things implementing an application port belong in `src/application/` or `src/infrastructure/`, not `src/daemon/`.

### Rule 7 — Presentation stays in CLI

Step tracking, progress display, formatting — these belong in `src/cli/`. Use cases return results; the CLI layer decides presentation. Don't pass `Vec<(String, StepOutcome)>` through use cases.

## Architecture checklist

- [ ] New code lives in the right layer (domain / application / infrastructure / cli / daemon / desktop)
- [ ] `src/domain/` has no `use crate::infrastructure::*` and no external crate imports beyond `std`, `thiserror`, `anyhow`
- [ ] Application ports return domain types or application DTOs, not infrastructure types
- [ ] New infrastructure providers added without touching `src/domain/`
- [ ] No setters exposing entity internals to infrastructure
- [ ] `src/daemon/` contains no application-level orchestration
- [ ] Use case results leave presentation (formatting, progress) to the CLI layer
