# Rodin project importer

The **Open in Rodin** command needs to register the project it exports into a
fresh Rodin workspace *before* launching Rodin, so the project shows up without
a manual `File → Import`. The robust, version-independent way to do that is to
ask Eclipse itself, via its headless Ant runner
(`org.eclipse.ant.core.antRunner`, bundled with Rodin), to load the `.project`
descriptor and create/open the project through the Eclipse Resources API.

That work is done by a tiny Ant task, [`RodinProjectImportTask.java`](./RodinProjectImportTask.java).

## Why a precompiled class is embedded

The extension cannot assume a JDK is installed on the user's machine, so it
**ships the compiled class** rather than compiling on demand. The class is
embedded as base64 in
[`../src/rossiCommands.ts`](../src/rossiCommands.ts) as
`RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64`. At runtime the extension writes it to
a temporary `org/rossi/vscode/RodinProjectImportTask.class`, generates a small
`build.xml`, and runs it through Rodin's Ant runner.

The base64 constant in `rossiCommands.ts` is the **canonical** copy. The files
in this directory exist only for review and reproducibility and are **not**
compiled or bundled by any build step (the `rodin/` directory is listed in
`.vscodeignore`, so it never ships in the `.vsix`):

- `RodinProjectImportTask.java` — the human-readable source.
- `RodinProjectImportTask.class` — the exact bytes of the embedded base64.

## Verifying the committed class matches the embedded base64

```bash
# Should print nothing (the decoded constant equals the committed .class).
node -e "const s=require('fs').readFileSync('editors/vscode/src/rossiCommands.ts','utf8'); \
  const m=s.match(/RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64 = \[(.*?)\]\.join\(''\)/s); \
  const b64=[...m[1].matchAll(/'([^']*)'/g)].map(x=>x[1]).join(''); \
  require('fs').writeFileSync('/tmp/embedded.class', Buffer.from(b64,'base64'));" \
  && diff /tmp/embedded.class editors/vscode/rodin/RodinProjectImportTask.class
```

## Regenerating

Requires a JDK 17 and the Apache Ant + Eclipse Platform resources/runtime APIs
that Rodin bundles (their exact bundle versions vary by Rodin release):

```bash
# 1. Compile against Rodin's bundled jars (adjust the plugins path/versions).
RODIN_PLUGINS=/Applications/Rodin.app/Contents/Eclipse/plugins   # macOS example
mkdir -p out/org/rossi/vscode
javac --release 17 -d out \
  -cp "$RODIN_PLUGINS/org.apache.ant_*/lib/ant.jar:\
$RODIN_PLUGINS/org.eclipse.core.resources_*.jar:\
$RODIN_PLUGINS/org.eclipse.core.runtime_*.jar:\
$RODIN_PLUGINS/org.eclipse.equinox.common_*.jar" \
  editors/vscode/rodin/RodinProjectImportTask.java
cp out/org/rossi/vscode/RodinProjectImportTask.class editors/vscode/rodin/

# 2. Re-embed: base64-encode and wrap into the array literal in rossiCommands.ts.
base64 editors/vscode/rodin/RodinProjectImportTask.class | fold -w 96
```

Paste the wrapped lines (each quoted, comma-separated) into
`RODIN_PROJECT_IMPORT_TASK_CLASS_BASE64`.
