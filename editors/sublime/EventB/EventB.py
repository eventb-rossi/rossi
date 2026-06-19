# Event-B input method plugin for Sublime Text 4.
#
# Provides eager (ASCII→Unicode as you type) and \name leader substitution.
#
# Install: copy the EventB/ directory (EventB.py, operators.py,
# EventB.sublime-syntax) into your Packages/ directory so Sublime Text sees
# Packages/EventB/EventB.py.  Both files must be in the same package
# directory so the relative import of operators.py works.
#
# Sublime Text auto-loads any .py file in a Packages/<name>/ directory;
# EventB.py activates for every view whose syntax is EventB.sublime-syntax.
#
# Two trigger modes (both on by default):
#   eager  — symbolic combos (=>, |->, <=>) convert via maximal munch.
#   leader — a \name abbreviation (\implies, \to, \nat) expands to Unicode
#             on the next boundary character.
#
# v1 scope: single primary cursor, character-by-character typing.  Paste,
# deletion, undo, and multi-cursor edits reset state and never corrupt the
# buffer.  Conversion happens everywhere, including inside comments
# (comment-awareness is a future refinement).

from __future__ import annotations

import re
import sublime
import sublime_plugin

try:
    from .operators import OPERATOR_ROWS
except ImportError:
    # Installed flat (Packages/User/), not as a package: operator data not
    # available, so the input method silently stays inactive.
    OPERATOR_ROWS = []

# ---------------------------------------------------------------------------
# Pure matcher
# ---------------------------------------------------------------------------

LEADER = "\\"


def _is_name_char(ch: str) -> bool:
    return ch.isalnum()


class _EagerMatcher:
    """Lookup tables built from OPERATOR_ROWS for O(1) eager and leader matching."""

    def __init__(self, rows: list) -> None:
        self._exact: dict[str, str] = {}   # ascii -> unicode (eager ops only)
        self._prefixes: set[str] = set()   # strict prefixes of eager op ASCIIs
        self._leader: dict[str, str] = {}  # leader name -> unicode
        for row in rows:
            if row["eager"]:
                self._exact[row["ascii"]] = row["unicode"]
                for i in range(1, len(row["ascii"])):
                    self._prefixes.add(row["ascii"][:i])
            for alias in row["aliases"]:
                self._leader[alias] = row["unicode"]
            # Alphabetic ASCII words (NAT, or, POW …) also work as leader names
            # so \NAT / \or expand alongside the curated aliases.
            if not row["symbolic"]:
                self._leader[row["ascii"]] = row["unicode"]
        self.max_leader_len: int = max(
            (len(k) for k in self._leader), default=0
        )

    def can_extend(self, s: str) -> bool:
        """True if some eager op is strictly longer than s and starts with s."""
        return s in self._prefixes

    def exact(self, s: str) -> str | None:
        """The Unicode glyph if s is exactly an eager op, else None."""
        return self._exact.get(s)

    def is_eager_start(self, s: str) -> bool:
        """True if s could begin or extend into an eager op."""
        return s in self._prefixes or s in self._exact

    def resolve_leader(self, name: str) -> str | None:
        """The Unicode glyph for a leader name, else None."""
        return self._leader.get(name)


_matcher: _EagerMatcher | None = None


def _get_matcher() -> _EagerMatcher | None:
    global _matcher
    if _matcher is None and OPERATOR_ROWS:
        _matcher = _EagerMatcher(OPERATOR_ROWS)
    return _matcher


# ---------------------------------------------------------------------------
# Eager state machine
# ---------------------------------------------------------------------------
# Action types:
#   {"type": "wait",           "pending": str}
#   {"type": "reset",          "pending": str}
#   {"type": "convertWithChar","unicode": str}
#   {"type": "convertHeld",    "unicode": str, "heldLen": int}


def _step_eager(matcher: _EagerMatcher, pending: str, ch: str) -> dict:
    """
    Advance the eager state machine by one typed character ch, given the
    current uncommitted ASCII pending run.  Pure function; no side effects.
    """
    # The leader char is reserved; it never participates in an eager run.
    if ch == LEADER:
        return {"type": "reset", "pending": ""}

    candidate = pending + ch

    if matcher.can_extend(candidate):
        return {"type": "wait", "pending": candidate}

    exact_candidate = matcher.exact(candidate)
    if exact_candidate is not None:
        return {"type": "convertWithChar", "unicode": exact_candidate}

    # candidate cannot extend and is not itself an op.  If the run we held
    # (without ch) was a complete op, ch is its boundary: convert the run.
    held_glyph = matcher.exact(pending)
    if held_glyph is not None:
        return {
            "type": "convertHeld",
            "unicode": held_glyph,
            "heldLen": len(pending),
        }

    # Restart, seeding the new run with ch if it could start an op.
    return {
        "type": "reset",
        "pending": ch if matcher.is_eager_start(ch) else "",
    }


# ---------------------------------------------------------------------------
# Leader helpers
# ---------------------------------------------------------------------------

_LEADER_TOKEN_RE = re.compile(r"\\([A-Za-z][A-Za-z0-9]*)$")


def _leader_token_before(text: str) -> tuple[str, int] | None:
    """
    Find a \\name leader token ending at the end of text (immediately before
    the cursor).  Returns (name, backslash_offset) or None.
    """
    m = _LEADER_TOKEN_RE.search(text)
    if m is None:
        return None
    return m.group(1), m.start()


# ---------------------------------------------------------------------------
# Replacement TextCommand (the edit gateway)
# ---------------------------------------------------------------------------


class EventbEagerReplaceCommand(sublime_plugin.TextCommand):
    """
    Replace the document region [start, end) with replacement as a single
    undo step.  The _rossi_applying guard in view settings ensures the
    resulting on_text_changed notification is ignored by EventBInputListener.
    """

    def run(self, edit: sublime.Edit, start: int, end: int, replacement: str) -> None:
        self.view.replace(edit, sublime.Region(start, end), replacement)


# ---------------------------------------------------------------------------
# ViewEventListener (editor glue)
# ---------------------------------------------------------------------------


class EventBInputListener(sublime_plugin.ViewEventListener):
    """
    As-you-type ASCII→Unicode substitution for Event-B files.

    Attaches to every view whose syntax is EventB.sublime-syntax and drives
    the eager and leader input modes on every single-character insertion.
    """

    @classmethod
    def is_applicable(cls, settings: sublime.Settings) -> bool:
        return settings.get("syntax", "").endswith("EventB.sublime-syntax")

    def __init__(self, view: sublime.View) -> None:
        super().__init__(view)
        # The uncommitted ASCII run sitting in the document immediately before
        # the cursor.
        self._pending: str = ""
        # Document point where the cursor sits after the last run extension
        # (None when the run is empty).  Used for the contiguity check.
        self._pending_pt: int | None = None
        self._applying: bool = False
        # Buffer size after the last observed modification.  Used to detect
        # single-character insertions without relying on command_history
        # (on_modified fires inside the insert command before it is committed
        # to the undo stack, so command_history returns the previous command).
        self._prev_size: int = view.size()

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _reset_pending(self) -> None:
        self._pending = ""
        self._pending_pt = None

    def _seed_after_commit(
        self,
        m: _EagerMatcher,
        ch: str,
        commit_end: int,
    ) -> None:
        """Seed ch into a fresh eager run after a leader or convertHeld commit.

        commit_end is the document point immediately after the replacement glyph
        (= backslash_pt + 1 for a leader commit, or held_start + 1 for
        convertHeld).  ch sits there; the cursor sits one beyond it.
        """
        new_insert = commit_end
        new_cursor = commit_end + 1
        act = _step_eager(m, "", ch)
        if act["type"] == "convertWithChar":
            self._replace(new_insert, new_cursor, act["unicode"])
        else:
            self._pending = act["pending"]
            self._pending_pt = new_cursor if act["pending"] else None

    def _replace(self, start: int, end: int, replacement: str) -> None:
        self._applying = True
        try:
            self.view.run_command(
                "eventb_eager_replace",
                {"start": start, "end": end, "replacement": replacement},
            )
        finally:
            self._applying = False

    # ------------------------------------------------------------------
    # Event hook
    # ------------------------------------------------------------------

    def on_modified(self) -> None:
        # Always update the size tracker first so it stays accurate even for
        # our own replacements (on_modified fires inside view.run_command,
        # before the command is committed to the undo stack).
        new_size: int = self.view.size()
        delta: int = new_size - self._prev_size
        self._prev_size = new_size

        if self._applying:
            return

        # Only single-character net insertions (not paste, delete, undo …).
        # command_history is unusable here: on_modified fires while the insert
        # command is still executing, before it appears in the undo history.
        if delta != 1:
            self._reset_pending()
            return

        sel = self.view.sel()
        if len(sel) != 1 or not sel[0].empty():
            self._reset_pending()
            return

        cursor_pt: int = sel[0].begin()
        if cursor_pt == 0:
            self._reset_pending()
            return

        insert_pt: int = cursor_pt - 1  # document point where ch landed
        ch: str = self.view.substr(insert_pt)
        # Only printable ASCII; multi-byte Unicode inserts (e.g. pasting a
        # glyph) reset state.
        if not ('\x20' <= ch <= '\x7e'):
            self._reset_pending()
            return

        m = _get_matcher()
        if m is None:
            # operators.py not available (flat install); stay inactive.
            return

        # Contiguity check: carry the pending run only when this insertion is
        # exactly adjacent to where the last run ended.
        if self._pending_pt is not None and insert_pt != self._pending_pt:
            self._reset_pending()

        # ------------------------------------------------------------------
        # Leader commit
        # Try to commit a \name abbreviation that ends right before ch when ch
        # is a boundary (not a name char, not the leader).
        # ------------------------------------------------------------------
        if not _is_name_char(ch) and ch != LEADER:
            line_start: int = self.view.line(insert_pt).begin()
            lookback: int = m.max_leader_len + 1
            window: str = self.view.substr(
                sublime.Region(max(line_start, insert_pt - lookback), insert_pt)
            )
            tok = _leader_token_before(window)
            if tok is not None:
                name, rel_start = tok
                glyph = m.resolve_leader(name)
                if glyph is not None:
                    # rel_start is the backslash offset within window; convert
                    # to a document point.
                    backslash_pt: int = insert_pt - len(window) + rel_start
                    self._reset_pending()
                    self._replace(backslash_pt, insert_pt, glyph)
                    # Seed ch into a fresh eager run so combos like \in:= work.
                    self._seed_after_commit(m, ch, backslash_pt + 1)
                    return

        # ------------------------------------------------------------------
        # Eager substitution
        # ------------------------------------------------------------------
        action = _step_eager(m, self._pending, ch)
        atype = action["type"]

        if atype in ("wait", "reset"):
            self._pending = action["pending"]
            self._pending_pt = cursor_pt if action["pending"] else None

        elif atype == "convertWithChar":
            # Replace the pending run plus the just-inserted ch with the glyph.
            # The pending chars occupy [insert_pt - len(pending), insert_pt);
            # ch occupies [insert_pt, cursor_pt).  Together: [start, cursor_pt).
            start: int = insert_pt - len(self._pending)
            self._reset_pending()
            self._replace(start, cursor_pt, action["unicode"])

        elif atype == "convertHeld":
            # Replace only the held run BEFORE ch with the glyph; keep ch.
            # The held run occupies [insert_pt - heldLen, insert_pt).
            start = insert_pt - action["heldLen"]
            self._reset_pending()
            self._replace(start, insert_pt, action["unicode"])
            # Seed ch into a fresh eager run so combos like <=:= work.
            self._seed_after_commit(m, ch, start + 1)
