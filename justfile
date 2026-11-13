set shell := ["bash", "-euo", "pipefail", "-c"]

# Show available commands
default:
    @just --list

# Run the app against the real Podcasts library and drives
start *args:
    cargo run -p castoff -- {{args}}
alias s := start

# Run the app with fake data (touches nothing real)
demo *args:
    cargo run -p castoff -- --demo {{args}}

# Run the app with the debug log enabled (F1)
debug *args:
    cargo run -p castoff -- --debug {{args}}
alias d := debug

# Build (debug)
build:
    cargo build --workspace
alias b := build

# Build an optimised binary at target/release/castoff
release:
    cargo build --release -p castoff

# Run tests (e.g. `just test -p castoff-core`, `just test sync`)
test *args:
    cargo test --workspace {{args}}
alias t := test

# Type check
check:
    cargo check --workspace --all-targets
alias c := check

# Lint code
lint:
    cargo clippy --workspace --all-targets
alias l := lint

# Format code
fmt:
    cargo fmt --all
alias f := fmt

# Run check, lint and tests
full-check: check lint test
alias fc := full-check

# List the headless scenes
scenes: build
    target/debug/castoff --list-scenes

# Render one scene headlessly to out/<scene>.png (e.g. `just snap transfer`)
snap scene="default" *args: build
    mkdir -p out
    target/debug/castoff --snap out/{{scene}}.png --scene {{scene}} {{args}}

# Render every scene headlessly to out/
snap-all: build
    mkdir -p out
    for s in $(target/debug/castoff --list-scenes); do target/debug/castoff --snap "out/$s.png" --scene "$s"; done

# Print what the engine finds on this Mac (read-only): settings, episodes, drives
probe:
    cargo run -p castoff-core --example probe

# Remove build artifacts and renders
clean:
    cargo clean
    rm -rf out

# Record assets/demo.gif headlessly from assets/demo.json (needs ffmpeg)
demo-gif: build
    rm -rf out/demo-frames
    target/debug/castoff --script assets/demo.json --out out/demo-frames --every 8 --size 1100x700 --scale 1 --keys bottom-right
    ffmpeg -y -loglevel error -framerate 7.5 -pattern_type glob -i "out/demo-frames/frame_*.png" -vf "split[a][b];[a]palettegen=max_colors=128:stats_mode=full[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" assets/demo.gif

# Build target/Castoff.app (release, ad-hoc signed)
app:
    scripts/bundle.sh

# Build Castoff.app and copy it to /Applications
install:
    scripts/bundle.sh --install
