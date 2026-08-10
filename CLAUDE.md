@AGENTS.md

## Claude-specific

- Use plan mode before anything that spans crate boundaries — the missing
  top-level workspace is a design decision, not an oversight.
- Check `df -h /System/Volumes/Data` before a broad `cargo` run on the Mac.
  Each crate has its own `target/`; an ENOSPC here kills the shell.
