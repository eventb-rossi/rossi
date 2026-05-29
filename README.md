# Rossi

A Rust toolchain for the Event-B formal modeling language: parser,
static checker, command-line tool, and Language Server Protocol
implementation.

## Overview

Event-B is a formal method for system-level modeling and analysis.
Rossi covers the full author-to-Rodin path:

- **`rossi`** — pest-based parser and typed AST with a pretty-printer
  that round-trips between `.eventb` text and the native Rodin
  `.buc` / `.bum` / `.zip` XML formats.
- **`rossi-build`** — static checker that layers type inference and
  well-formedness checks on the AST and emits Rodin-compatible
  `.bcc` / `.bcm` checked XML, so models authored in text round-trip
  through the Rodin toolchain.
- **`rossi-cli`** — the `rossi` command-line tool wrapping the
  parser, checker, and language server.
- **`rossi-lsp`** — Language Server Protocol implementation powering
  editor extensions for VS Code, Neovim, and Emacs.

## Features

**Complete Event-B Syntax Support**
- Contexts (sets, constants, axioms, theorems)
- Machines (variables, invariants, events, variants)
- Mathematical expressions and predicates
- Event refinement, convergence, and witnesses
- Set theory and first-order logic operators
- Native XML format (`.buc` and `.bum` files from Rodin)

**Modern Parser Architecture**
- Built with [pest](https://pest.rs/) PEG parser generator
- Type-safe AST with Rust's strong type system
- Detailed error messages with source locations
- Syntax error recovery for partial parsing
- Optional serde support for JSON serialization
- Support for both text and XML Event-B formats

**Developer Friendly**
- Comprehensive test suite
- Well-documented API
- Example Event-B models included
- Clean separation of grammar, AST, and parser logic
- Pretty printer for AST-to-text conversion
- Roundtrip support (parse, transform, print)

For editor and Language Server features, see
[Language Server & IDE Support](#language-server--ide-support) below.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
rossi = "0.1"
```

Or install from source:

```bash
git clone https://github.com/eventb-rossi/rossi
cd rossi
cargo build --release
```

## Quick Start

### Parsing Event-B

```rust
use rossi::parse;

fn main() {
    let source = r#"
        CONTEXT counter_ctx
        CONSTANTS
            max_value
        AXIOMS
            axm1: max_value = 100
        END
    "#;

    match parse(source) {
        Ok(component) => {
            println!("Successfully parsed: {:#?}", component);
        }
        Err(e) => {
            eprintln!("Parse error: {}", e);
        }
    }
}
```

### Parsing Native Event-B XML Format

The parser also supports the native Event-B XML format used by the Rodin platform (`.buc` and `.bum` files):

```rust
use rossi::parse_xml;
use std::fs;

fn main() {
    // Parse a .buc context file
    let xml = fs::read_to_string("model.buc").unwrap();
    let component = parse_xml(&xml).unwrap();

    println!("Parsed: {:#?}", component);
}
```

Supported XML file types:
- **`.buc` files**: Event-B contexts (compatible with Rodin 3.0+)
- **`.bum` files**: Event-B machines (compatible with Rodin 3.0+)
- **`.zip` archives**: Complete Rodin projects containing multiple .buc and .bum files

The XML parser automatically converts the native format into the same AST structure as the text parser, allowing seamless interoperability between formats.

### Parsing Rodin Project Archives

Rodin stores complete Event-B projects as ZIP archives. You can parse all components from a ZIP file at once:

```rust
use rossi::parse_zip_file;

// Parse all components from a Rodin project archive
let components = parse_zip_file("myproject.zip").unwrap();

for comp in &components {
    println!("Found component: {} from file {}",
        match &comp.component {
            rossi::Component::Context(c) => &c.name,
            rossi::Component::Machine(m) => &m.name,
        },
        comp.filename
    );
}
```

### Pretty Printing

Convert AST back to Event-B text:

```rust
use rossi::{parse, to_string};

fn main() {
    let source = "CONTEXT test\nSETS\n    STATUS\nEND\n";

    // Parse to AST
    let component = parse(source).unwrap();

    // Convert back to text
    let output = to_string(&component);
    println!("{}", output);
}
```

The pretty printer supports both Unicode (default) and ASCII operators:

```rust
use rossi::{parse, to_string, to_string_ascii, PrettyPrinter};

let component = parse(source).unwrap();

// Unicode output (default)
let unicode_output = to_string(&component);

// ASCII output
let ascii_output = to_string_ascii(&component);

// Custom configuration
let printer = PrettyPrinter::new()
    .with_indent("  ".to_string());
let custom_output = printer.print_component(&component);
```

## Event-B Syntax Support

### Contexts

Contexts define static properties of a model:

```eventb
CONTEXT library_ctx
EXTENDS
    base_ctx
SETS
    BOOK, READER
CONSTANTS
    max_loans
AXIOMS
    axm1: max_loans = 5
    axm2: max_loans > 0
END
```

### Machines

Machines define dynamic behavior through events:

```eventb
MACHINE counter
SEES
    counter_ctx
VARIABLES
    count
INVARIANTS
    inv1: count >= 0
    inv2: count <= max_value
EVENTS
    INITIALISATION
    BEGIN
        count := 0
    END

    EVENT increment
    WHERE
        grd1: count < max_value
    THEN
        count := count + 1
    END

    EVENT reset
    THEN
        count := 0
    END
END
```

### Mathematical Operators

The parser supports both Unicode and ASCII alternatives following the
[Rodin Keyboard](https://wiki.event-b.org/index.php/Rodin_Keyboard_User_Guide) conventions:

| Operator | Unicode | ASCII | Description |
|----------|---------|-------|-------------|
| And | `∧` | `&` | Logical conjunction |
| Or | `∨` | `or` | Logical disjunction |
| Implies | `⇒` | `=>` | Implication |
| Not | `¬` | `not` | Negation |
| In | `∈` | `:` | Set membership |
| Subset | `⊆` | `<:` | Subset or equal |
| Union | `∪` | `\/` | Set union |
| Intersection | `∩` | `/\` | Set intersection |
| Cartesian | `×` | `**` | Cartesian product |
| Power set | `ℙ` | `POW` | Power set |
| Empty set | `∅` | `{}` | Empty set |
| Forall | `∀` | `!` | Universal quantifier |
| Exists | `∃` | `#` | Existential quantifier |
| Maplet | `↦` | `\|->` | Ordered pair |

Note that logical AND/OR (`&`, `or`) and set intersection/union (`/\`, `\/`)
use different ASCII representations.

## AST Structure

The parser produces a strongly-typed AST:

```rust
pub enum Component {
    Context(Context),
    Machine(Machine),
}

pub struct Context {
    pub name: String,
    pub extends: Vec<String>,
    pub sets: Vec<SetDeclaration>,
    pub constants: Vec<NamedElement>,
    pub axioms: Vec<LabeledPredicate>,
    // ... source location and metadata fields
}

pub struct Machine {
    pub name: String,
    pub refines: Option<String>,
    pub sees: Vec<String>,
    pub variables: Vec<NamedElement>,
    pub invariants: Vec<LabeledPredicate>,
    pub variant: Option<Expression>,
    pub initialisation: Option<InitialisationEvent>,
    pub events: Vec<Event>,
    // ... source location and metadata fields
}
```

`SetDeclaration` supports both deferred (carrier) sets and enumerated sets.
`NamedElement` carries a name and an optional comment (from Rodin XML).
Theorem predicates are stored in `axioms` or `invariants` with
`is_theorem = true`.

## CLI Tool

The project ships a `rossi` command-line tool that wraps the parser,
the `rossi-build` static checker, and the language server:

| Subcommand | Purpose |
|------------|---------|
| `validate` | Validate `.eventb` files, Rodin `.zip` archives, or unzipped Rodin project directories. |
| `import`   | Import Rodin `.zip`/`.buc`/`.bum`/dir into `.eventb` text. |
| `export`   | Export `.eventb`/`.txt`/dir into a Rodin `.zip` archive. |
| `fmt`      | Reformat Event-B in place (operator convention, indentation). |
| `build`    | Static-check a Rodin project and emit `.bcc` / `.bcm` checked XML. |
| `lsp`      | Run the Rossi language server over stdio (equivalent to the `rossi-language-server` binary). |

### Installation

```bash
cargo build --release -p rossi-cli
```

The binary will be available at `target/release/rossi`.

### Validate

```bash
# Validate a single file
rossi validate crates/rossi/examples/counter.eventb

# Validate multiple files
rossi validate crates/rossi/examples/*.eventb

# JSON output for tooling integration
rossi validate --format json crates/rossi/examples/counter.eventb

# SARIF output for IDEs and code-scanning tools
rossi validate --format sarif crates/rossi/examples/base-model.zip

# Quiet mode (only show errors)
rossi validate --quiet crates/rossi/examples/*.eventb

# Continue past failures
rossi validate --continue-on-error crates/rossi/examples/*.eventb

# Skip semantic checks for .zip inputs, or skip advisory lints
rossi validate --no-semantic crates/rossi/examples/base-model.zip
rossi validate --no-lints crates/rossi/examples/base-model.zip
```

**Text output:**
```
✓ crates/rossi/examples/counter.eventb - Valid Context 'counter_ctx'
✓ crates/rossi/examples/counter_machine.eventb - Valid Machine 'counter'

==================================================
Summary:
  Total:  2
  Passed: 2 ✓
  Failed: 0 ✗
==================================================
```

**JSON output:**
```json
[
  {
    "file": "crates/rossi/examples/counter.eventb",
    "success": true,
    "component_type": "Context",
    "component_name": "counter_ctx"
  }
]
```

For `.eventb` files, `validate` parses the text and reports component results.
For `.zip` archives, it also runs rossi-build semantic checks and advisory
lints unless `--no-semantic` is set; `--no-lints` keeps semantic checks but
drops advisory lint rows. Directory inputs are treated as unzipped Rodin
projects and require semantic checks, so `--no-semantic` is rejected for them.

### Import (Rodin → Event-B text)

```bash
# Convert a Rodin .zip archive into .eventb text files (one per component)
rossi import project.zip --output ./project

# Use ASCII operators (and a custom indent) in the emitted text
rossi import project.zip --output ./project --ascii --indent="  "

# Merge all components into a single file, optionally specifying order
rossi import project.zip --output project.eventb --merge=M0,C0
```

### Export (Event-B text → Rodin ZIP)

```bash
# Pack a directory of .eventb files into a Rodin .zip archive
rossi export ./project --output project.zip
```

The archive always uses Unicode operators, which is what Rodin expects, so
`export` has no operator-convention option — use `rossi fmt` to change the
convention of text files.

### Format (`fmt`)

`fmt` reformats Event-B *without* crossing the Rodin↔text boundary: it
normalizes the operator convention (`--ascii`/`--unicode`, default Unicode) and
indentation (`--indent`).

```bash
# Convert ASCII-operator text to Unicode (default), printing to stdout
rossi fmt model.eventb

# Reformat files in place; pick the operator convention explicitly
rossi fmt -i ./project --ascii
rossi fmt -i model.eventb --indent="  "

# CI gate: exit non-zero if anything is not already formatted
rossi fmt --check ./project

# Normalize a Rodin archive to canonical Unicode XML (other entries preserved)
rossi fmt project.zip -o normalized.zip
```

Editors using the language server format on save with the same engine; `rossi
fmt` is its command-line and CI counterpart. (Rodin archives must stay Unicode,
so `--ascii` is rejected for `.zip`/`.buc`/`.bum` inputs.)

### Build (static check + Rodin checked XML)

```bash
# Static-check and emit .bcc / .bcm into a checked Rodin .zip
rossi build project.zip --output project-checked.zip

# Or emit loose files into a directory
rossi build project.zip --output ./out
```

### LSP

```bash
# Start the language server over stdio
rossi lsp
```

This is identical to running the standalone `rossi-language-server`
binary; editor extensions may invoke either form.

### Exit Codes

- `0` - All files validated successfully
- `1` - One or more files failed validation or file not found

## Examples

See the `crates/rossi/examples/` directory for Event-B model files:

- `counter.eventb` / `counter_machine.eventb` - Simple counter
- `library_ctx.eventb` / `library_machine.eventb` - Library management
- `traffic_light_ctx.eventb` / `traffic_light_machine.eventb` - Traffic light controller
- `scheduler_ctx.eventb` / `scheduler_machine.eventb` - Process scheduler
- `refinement_abstract.eventb` / `refinement_concrete.eventb` - Refinement example
- `lambda_example_ctx.eventb` - Lambda expressions and set comprehensions

## Testing

The project includes comprehensive tests:

```bash
# Run all tests
cargo test

# Run specific test suite
cargo test --test full_models_test

# Run with output
cargo test -- --nocapture
```

## Architecture

The project is organized as a Cargo workspace with four public crates:

```
rossi/
├── crates/
│   ├── rossi/        # Core parser, AST, pretty-printer, Rodin XML
│   ├── rossi-build/  # Static checker / Rodin .bcc / .bcm builder
│   ├── rossi-cli/    # `rossi` command-line interface
│   └── rossi-lsp/    # Language Server Protocol implementation
└── editors/
    ├── vscode/       # VS Code extension
    ├── neovim/       # Neovim plugin
    └── emacs/        # Emacs major mode
```

## Development

### Setup

```bash
# Enable pre-commit hook (runs cargo fmt, clippy, and tests)
git config core.hooksPath .githooks
```

### Building

```bash
cargo build
```

### Running Tests

```bash
cargo test
```

### Documentation

```bash
cargo doc --open
```

## Language Server & IDE Support

The project includes a fully-featured **Language Server Protocol (LSP)** implementation that provides modern IDE features for Event-B development.

### Implemented Features

- **Real-time diagnostics** - Syntax errors as you type with error recovery
- **Code completion** - Context-aware keywords, operators, identifiers, and snippets
- **Hover documentation** - Keyword, operator, and identifier information
- **Go-to-definition** - Navigate to declarations across files
- **Find references** - Workspace-wide reference search
- **Rename refactoring** - Safe identifier renaming with validation
- **Document symbols** - Hierarchical outline and breadcrumb navigation
- **Code formatting** - Auto-format with Unicode or ASCII operators
- **Semantic highlighting** - AST-based token classification
- **Code actions** - Unicode/ASCII conversion, extract constant, sort clauses
- **Signature help** - Quantifier and lambda parameter hints
- **Document links** - Clickable SEES/REFINES/EXTENDS references
- **Code folding** - Collapse contexts, machines, events, and clause sections
- **Cross-file resolution** - Transitive SEES/REFINES/EXTENDS chain traversal
- **ProB integration** - Code lenses for model checking (when ProB is installed)

### Editor Extensions

Extensions are available in the `editors/` directory:

- **VS Code** (`editors/vscode/`) - Syntax highlighting, LSP integration, snippets
- **Neovim** (`editors/neovim/`) - File detection, syntax highlighting, LSP config
- **Emacs** (`editors/emacs/`) - Major mode for Event-B

See each editor's README and INSTALL files for setup instructions.

## Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all checks pass: `cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
5. Submit a pull request

## Related Projects

- [Rodin Platform](https://www.event-b.org/) - Eclipse-based IDE for Event-B
- [ProB](https://prob.de/) - Animator and model checker for Event-B
- [Event-B Documentation](https://wiki.event-b.org/)

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## References

- [Event-B Language Summary](https://wiki.event-b.org/index.php/Event-B_Language)
- [Event-B Notation Guide](https://wiki.event-b.org/index.php/Mathematical_Notation)
- [Rodin Keyboard User Guide](https://wiki.event-b.org/index.php/Rodin_Keyboard_User_Guide)
- [Rodin User Manual](https://wiki.event-b.org/index.php/Rodin_User_Manual)

## Authors

Rossi Contributors
