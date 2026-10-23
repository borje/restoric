# Re-record docs/demo.gif with the current source. Needs vhs (brew install vhs).
demo:
    cargo build --release
    PATH="$PWD/target/release:$PATH" vhs docs/demo.tape

# Re-record, then commit and push the gif. The README and GitHub Pages
# (served from main:/docs) both show docs/demo.gif, so the push updates both.
publish-demo: demo
    git diff --quiet docs/demo.gif || git commit -m "Update demo gif" docs/demo.gif
    git push
