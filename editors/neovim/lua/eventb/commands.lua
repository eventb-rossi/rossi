-- Rossi user commands for Event-B buffers.
-- https://github.com/eventb-rossi/rossi
--
-- Mirrors the VS Code command controller (editors/vscode/src/rossiCommands.ts)
-- so Neovim and VS Code drive the same `rossi` CLI / LSP plumbing.
--
-- The `rossi` binary is configurable via `vim.g.rossi_tool_path` (default
-- "rossi"); set it before this module is loaded, e.g.
--   vim.g.rossi_tool_path = vim.fn.expand("~/.cargo/bin/rossi")

local M = {}

-- Dedicated diagnostic namespace so `:RossiValidate*` results never collide
-- with the LSP's live diagnostics.
local diag_ns = vim.api.nvim_create_namespace("rossi-validate")

-- Resolve the configured `rossi` executable lazily so changes to
-- `vim.g.rossi_tool_path` after load still take effect.
local function tool_path()
  return vim.g.rossi_tool_path or "rossi"
end

-- Map the JSON `severity` string from `rossi validate` to a vim.diagnostic
-- severity, mirroring diagnosticSeverity() in rossiCommands.ts (default Error).
local function severity_of(severity)
  if severity == "warning" then
    return vim.diagnostic.severity.WARN
  elseif severity == "info" then
    return vim.diagnostic.severity.INFO
  elseif severity == "hint" then
    return vim.diagnostic.severity.HINT
  end
  return vim.diagnostic.severity.ERROR
end

-- Compose the human-readable diagnostic message, mirroring validationMessage()
-- in rossiCommands.ts: "[rule_id] inner_filename: origin: error".
local function validation_message(row)
  local parts = {}
  if row.rule_id then
    table.insert(parts, "[" .. row.rule_id .. "]")
  end
  if row.inner_filename then
    table.insert(parts, row.inner_filename .. ":")
  end
  if row.origin then
    table.insert(parts, row.origin .. ":")
  end
  table.insert(parts, row.error or row.severity or "Validation issue")
  return table.concat(parts, " ")
end

-- Run `rossi <args>` capturing stdout/stderr. `opts.stdin` is piped to the
-- child; `opts.on_exit(result)` receives { code, stdout, stderr } on the main
-- loop. Returns nil and notifies if the binary cannot be started.
local function run_rossi(args, opts)
  opts = opts or {}
  local cmd = { tool_path() }
  vim.list_extend(cmd, args)

  local ok, handle = pcall(vim.system, cmd, {
    text = true,
    stdin = opts.stdin,
    cwd = opts.cwd,
  }, function(result)
    vim.schedule(function()
      opts.on_exit({
        code = result.code,
        stdout = result.stdout or "",
        stderr = result.stderr or "",
      })
    end)
  end)

  if not ok then
    vim.notify("Failed to start '" .. tool_path() .. "': " .. tostring(handle), vim.log.levels.ERROR)
    return nil
  end
  return handle
end

-- The .eventb file backing the current buffer, or nil with a notification.
local function current_eventb_file()
  local name = vim.api.nvim_buf_get_name(0)
  if name == "" then
    vim.notify("Open or select a .eventb file first.", vim.log.levels.ERROR)
    return nil
  end
  return name
end

-- Replace the whole current buffer with `text` as a single undo block,
-- mirroring replaceDocumentText() in rossiCommands.ts.
local function replace_buffer(bufnr, text)
  -- vim.system on Windows may hand back CRLF; split on either.
  local lines = vim.split(text, "\r?\n")
  -- A trailing newline produces an empty final element; drop it so we do not
  -- append a spurious blank line on every conversion.
  if #lines > 0 and lines[#lines] == "" then
    table.remove(lines)
  end
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
end

-- RossiConvertUnicode / RossiConvertAscii: pipe the buffer through
-- `rossi fmt - --unicode|--ascii` and replace it in place. (ref convertCurrentFile)
local function convert(ascii)
  local bufnr = vim.api.nvim_get_current_buf()
  local file = current_eventb_file()
  if not file then
    return
  end
  local buffer = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n")
  local flag = ascii and "--ascii" or "--unicode"

  run_rossi({ "fmt", "-", flag }, {
    stdin = buffer,
    cwd = vim.fs.dirname(file),
    on_exit = function(result)
      if result.code ~= 0 then
        vim.notify("rossi fmt failed: " .. result.stderr, vim.log.levels.ERROR)
        return
      end
      if not vim.api.nvim_buf_is_valid(bufnr) then
        return
      end
      replace_buffer(bufnr, result.stdout)
      vim.notify(
        "Converted " .. vim.fs.basename(file) .. " to " .. (ascii and "ASCII" or "Unicode") .. ".",
        vim.log.levels.INFO
      )
    end,
  })
end

-- Decode `rossi validate --format json` output and publish it onto `diag_ns`,
-- grouped by file. Mirrors applyValidationDiagnostics() in rossiCommands.ts.
local function apply_validation(stdout, cwd)
  local ok, rows = pcall(vim.json.decode, stdout)
  if not ok or type(rows) ~= "table" then
    vim.notify("Failed to parse rossi validation JSON.", vim.log.levels.ERROR)
    return
  end

  -- Clear previous results across all buffers before republishing.
  vim.diagnostic.reset(diag_ns)

  local by_buf = {}
  for _, row in ipairs(rows) do
    -- Skip rows that carry neither an error nor a severity (success entries).
    if row.error or row.severity then
      local target = row.file
      if target and not vim.startswith(target, "/") then
        target = vim.fs.joinpath(cwd, target)
      end
      -- inner_filename names the component inside a directory/archive.
      if target and row.inner_filename and not vim.endswith(target:lower(), ".zip") then
        target = vim.fs.joinpath(target, row.inner_filename)
      end

      local bufnr = vim.fn.bufadd(target or cwd)
      vim.fn.bufload(bufnr)
      by_buf[bufnr] = by_buf[bufnr] or {}
      table.insert(by_buf[bufnr], {
        lnum = 0,
        col = 0,
        message = validation_message(row),
        severity = severity_of(row.severity),
        source = "rossi",
        code = row.rule_id,
      })
    end
  end

  for bufnr, diags in pairs(by_buf) do
    vim.diagnostic.set(diag_ns, bufnr, diags)
  end
end

-- Run `rossi validate --format json --continue-on-error <inputs>` and surface
-- the diagnostics. Mirrors runValidate() in rossiCommands.ts.
local function run_validate(inputs, cwd, stdin)
  local args = { "validate", "--format", "json", "--continue-on-error" }
  vim.list_extend(args, inputs)

  run_rossi(args, {
    stdin = stdin,
    cwd = cwd,
    on_exit = function(result)
      apply_validation(result.stdout, cwd)
      if result.code == 0 then
        vim.notify("Rossi validation completed.", vim.log.levels.INFO)
      else
        vim.notify("Rossi validation found issues. See diagnostics.", vim.log.levels.WARN)
      end
    end,
  })
end

-- RossiValidate: validate the in-editor buffer via stdin so unsaved edits are
-- checked; `--stdin-filename` maps the diagnostics back to the document.
local function validate_current()
  local bufnr = vim.api.nvim_get_current_buf()
  local file = current_eventb_file()
  if not file then
    return
  end
  local buffer = table.concat(vim.api.nvim_buf_get_lines(bufnr, 0, -1, false), "\n")
  run_validate({ "--stdin-filename", file, "-" }, vim.fs.dirname(file), buffer)
end

-- RossiValidateWorkspace: validate the LSP workspace root (or cwd) on disk.
local function validate_workspace()
  local root = vim.lsp.buf.list_workspace_folders()[1] or vim.fn.getcwd()
  run_validate({ root }, root)
end

-- Prompt for a value with a default, returning nil if the user cancels.
local function prompt(label, default)
  local answer = vim.fn.input(label, default or "")
  if answer == "" then
    return nil
  end
  return answer
end

-- RossiImport / RossiExport / RossiBuild: prompt for input/output paths and run
-- `rossi import|export|build <path> -o <out>`. Mirrors the VS Code import/
-- export/build commands (which use file pickers instead of text prompts).
local function run_io(subcommand, input_label, default_input, out_label)
  local input = prompt(input_label, default_input)
  if not input then
    return
  end
  local output = prompt(out_label, "")
  if not output then
    return
  end

  run_rossi({ subcommand, input, "-o", output }, {
    on_exit = function(result)
      if result.code == 0 then
        vim.notify("rossi " .. subcommand .. " -> " .. output, vim.log.levels.INFO)
      else
        vim.notify("rossi " .. subcommand .. " failed: " .. result.stderr, vim.log.levels.ERROR)
      end
    end,
  })
end

-- Create every :Rossi* user command. Safe to call more than once.
function M.setup()
  local cmd = vim.api.nvim_create_user_command

  cmd("RossiConvertUnicode", function()
    convert(false)
  end, { desc = "Convert the current Event-B buffer to Unicode operators" })

  cmd("RossiConvertAscii", function()
    convert(true)
  end, { desc = "Convert the current Event-B buffer to ASCII operators" })

  cmd("RossiValidate", validate_current, { desc = "Validate the current Event-B buffer" })

  cmd("RossiValidateWorkspace", validate_workspace, { desc = "Validate the Event-B workspace" })

  cmd("RossiImport", function()
    run_io("import", "Rodin project to import: ", "", "Output directory: ")
  end, { desc = "Import a Rodin project into Event-B text" })

  cmd("RossiExport", function()
    local default = vim.api.nvim_buf_get_name(0)
    run_io("export", "Event-B input to export: ", default, "Output Rodin ZIP: ")
  end, { desc = "Export Event-B text to a Rodin ZIP" })

  cmd("RossiBuild", function()
    local default = vim.api.nvim_buf_get_name(0)
    run_io("build", "Input to build: ", default, "Output checked Rodin ZIP: ")
  end, { desc = "Build a checked Rodin ZIP" })
end

-- Register commands on require so manual `luafile`/`require` both work.
M.setup()

return M
