# Reference dump documents

The `.json` files here are what `rossi_build::dump::model` writes for three of
the models in `crates/rossi/examples/`. `dump_golden.rs` compares its output
against them byte for byte.

These differ in kind from the proof-obligation and proof references beside
them. Those compare rossi against Rodin, so they can say rossi is *right*.
Nothing but rossi produces this format, so these can only say rossi has not
*changed*. Whether the format is correct is settled by `dump_model.rs`, which
checks the document against the model it came from, and by `dump_schema.rs`,
which checks it against the published schema.

That makes their job drift detection. Any change to a field, an order, a name,
a span or a type string shows up here as a diff, so no such change reaches a
release without someone having looked at it.

## What the three cover

- `bank_account` — a context and a machine from Event-B text, the widest
  operator vocabulary of the three, and both simple and set-choice
  assignments.
- `refinement` — two machines, one refining the other: inherited invariants, a
  witness, and a such-that assignment with its after-state declarations.
- `binary-search` — a Rodin archive: an extended event chain with inherited
  guards, actions and invariants, a variant, a witness, and both the
  convergent and anticipated forms.

A fourth model was tried and dropped. It more than doubled the size of this
directory while adding one feature the schema tests already exercise, over a
smaller operator vocabulary.

## Regenerating

After a deliberate change to the format or to the checker:

```sh
ROSSI_DUMP_GOLDEN_REGENERATE=1 cargo test -p rossi-build --features serde \
    --test dump_golden documents_match
```

That rewrites every file here and then fails on purpose, so a regeneration is
never mistaken for a passing run. Review the diff before committing it: a
change to these files is a change to what external consumers receive.
