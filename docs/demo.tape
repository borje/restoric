# The README's demo, recorded with vhs (https://github.com/charmbracelet/vhs).
# Regenerate from the repository root:
#
#     vhs docs/demo.tape
#
# It builds the release binary first and drives `restoric demo`, which runs
# against the sample project in tests/fixtures/project.dsl, so the result
# depends only on the source, not on any repository or clock.

Output docs/demo.gif

Set Shell bash
Set FontSize 14
Set Width 1040
Set Height 580
Set Padding 12
Set Framerate 20
Set Theme "Catppuccin Mocha"
Set TypingSpeed 60ms
Env TZ UTC

Hide
Type "cargo build --release --quiet && clear" Enter
Wait+Line@120s
Show

Type "target/release/restoric demo --no-icons" Sleep 400ms Enter
Sleep 2.5s

# Move to main.go and step through the snapshots where it changed.
Type "jjj" Sleep 1.5s
Type "H" Sleep 1.2s
Type "H" Sleep 1.2s
Type "{" Sleep 1.5s

# The preview can diff the old version against what's on disk now.
Tab Sleep 2.5s

# A file opens its versions.
Enter Sleep 2.5s
Type "j" Sleep 1s
Type "j" Sleep 1.5s
Escape Sleep 1s

# Yank a version and put it next to the original.
Type "y" Sleep 1.5s
Type "p" Sleep 2.5s

Type "q" Sleep 500ms
