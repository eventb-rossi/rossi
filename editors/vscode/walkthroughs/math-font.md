# The Rodin math font

Four Event-B operators have no standard Unicode code point, so Rodin encodes
them in the Unicode Private Use Area: `<<->` (U+E100), `<->>` (U+E101),
`<<->>` (U+E102) and `<+` (U+E103).

Rossi writes them in ASCII, which renders in any font, so **most users never
need this font**. Install it if you turn on `rossi.format.privateUseGlyphs` to
exchange files with a tool that reads only Rodin's spelling, such as
Rodin's own editors.

**Rossi: Install the Rodin Math Font** copies the bundled font into your own
font directory — `~/Library/Fonts` on macOS, `~/.local/share/fonts` on Linux,
`%LOCALAPPDATA%\Microsoft\Windows\Fonts` on Windows — and registers it. No
administrator rights are needed on any platform. Restart VS Code afterwards.

The command then offers to list the font as a fallback for Event-B files,
adding it after your own font stack in your user settings — your font keeps
every glyph it has, and Brave Sans Mono covers the four it does not. Declining
changes nothing; you can always write it yourself:

```json
{
  "[eventb]": {
    "editor.fontFamily": "'Your Font', 'Brave Sans Mono', monospace"
  }
}
```

The font is Brave Sans Mono Roman, a Bitstream Vera Sans Mono derivative by
ETH Zurich, redistributed unmodified under the Bitstream Vera Fonts License.
