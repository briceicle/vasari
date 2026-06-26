# Hero GIF for the README, rendered with charmbracelet/vhs:
#   vhs docs/media/vasari-why.tape
#
# Prereqs: `vasari` on PATH, and a graph ingested into /tmp/vasari-demo, e.g.
#   vasari --repo /tmp/vasari-demo ingest claude-code <a-real-session>.jsonl
# (the GIF was recorded against Vasari's own development session, so the answer
# is a real attribution.)
Output docs/media/vasari-why.gif

Set Shell "zsh"
Set FontSize 16
Set Width 1280
Set Height 280
Set Padding 22
Set Theme "Catppuccin Mocha"
Set TypingSpeed 55ms

Hide
Type "export PATH=$HOME/.cargo/bin:$PATH && cd /tmp/vasari-demo && clear"
Enter
Show

Sleep 600ms
Type "vasari why crates/vasari-core/src/adapters/otel.rs:8"
Sleep 700ms
Enter
Sleep 4500ms
