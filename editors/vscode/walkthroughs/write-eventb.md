# Write Event-B

Open the `.eventb` file and start modelling:

- Type `context`, `machine`, `event`, `inv`, `axm`, … and accept a **snippet** to
  scaffold a block.
- Syntax errors are underlined **live** as you type.
- On save, the full `rossi validate` runs over the project — adding the type and
  dead-code checks the live server skips. Turn it off with `rossi.validate.onSave`.
- Use the **Outline** view and breadcrumbs to navigate contexts and machines.
- Switch operator style any time with **Convert to Unicode** (`Ctrl/Cmd+K U`) and
  **Convert to ASCII** (`Ctrl/Cmd+K A`).
