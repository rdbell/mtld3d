# Readback experiment snapshot, September 11, 2026

This branch preserves the previously uncommitted fused-readback experiment and its
within-run A/B mode. It also contains the earlier FFXI fixes it was developed on top
of. It is saved at the user's request to commit and push all outstanding work.

Use development for the validated build-25 renderer source, including the later
vertex-stride correction. This experiment was not installed or enabled for play
testing during publication. The earlier experiment decision remains unchanged:
matched tests did not demonstrate a worthwhile benefit from fused readback.

Publication checks used Rust 1.97.1 and a separate Wine SDK and test prefix. The
installed game prefix and app were not altered.

- Secret scan passed. Staged source whitespace check passed.
- make check failed at rustfmt on the existing experimental code. Later check
  stages therefore did not run.
- make test passed 1,043 core/type and 203 shared/native tests.
- The full test command reached its 240-second wall-clock limit during the i686
  rendering suite. Of 449 tests, 51 passed, four active tests were terminated by
  the deadline, and 394 did not run. The x86_64 rendering suite did not run.
  This is an incomplete run, not a passing end-to-end validation or four observed
  assertion failures.

The branch retains the experimental source rather than mixing an unsolicited
cleanup or a new optimization campaign into this preservation commit. It is not
ready for promotion to development. Raw captures and private rollback copies
remain outside Git.
