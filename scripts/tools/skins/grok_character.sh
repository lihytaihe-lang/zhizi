#!/bin/bash
# Have Cursor's Grok draw one animal character: a three-view reference sheet first, then one sprite sheet per action (magenta background).
# The art lives in ime_context_model/skin-art/v3/<id>/ (not in Git).  The animal artwork is Grok's, never drawn by code (owner rule).
#   scripts/tools/skins/grok_character.sh <id> "<English description of the animal>"
# Waits for each image (about 3-6 minutes each), retries once, and writes <action>.done / <action>.fail next to the images.
set -u
ID="$1"; DESC="$2"
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
DIR="$ROOT/ime_context_model/skin-art/v3/$ID"
mkdir -p "$DIR"; cd "$DIR" || exit 1
STYLE="$ROOT/ime_context_model/skin-art/v3/cat/style-reference-capybara.png"
[ -f style-reference.png ] || cp "$STYLE" style-reference.png
MODEL=grok-4.7-high
run() { # name, prompt
  local name="$1" prompt="$2" try
  for try in 1 2; do
    cursor-agent -p --force --trust --model "$MODEL" --workspace "$DIR" "$prompt" > "$name.log" 2>&1
    if [ -s "$name.png" ]; then echo ok > "$name.done"; return 0; fi
  done
  echo fail > "$name.fail"; return 1
}
COMMON="Use your image generation tool to create ONE image and save it in the current directory. Pure magenta #FF00FF flat background (no gradient, no shadow on the background, no text, no grid lines, no frame). Soft 3D-rendered cute mascot style like the attached style-reference.png (cartoon proportions, gentle shading, clean silhouette)."
run character-sheet "$COMMON Save as character-sheet.png. A character reference sheet of the SAME $DESC: three views side by side (front, side facing right, back), identical proportions, colours and face, consistent for animation."
SHEET="Use the attached character-sheet.png as the exact character reference: the SAME animal, same proportions, colours, face and style in every frame."
FRAME="Sprite sheet: frames laid out in a grid with equal-size cells, the character facing right, its feet on the same ground line in every cell, no overlapping between cells, every frame fully inside its cell."
run stand "$COMMON $SHEET Save as stand.png. $FRAME An idle STAND loop, 4 frames in a 2x2 grid: standing, chest rising slightly, one blink."
run walk "$COMMON $SHEET Save as walk.png. $FRAME A WALK cycle, 8 frames in a 4x2 grid, facing right (contact, down, passing, up)."
run run "$COMMON $SHEET Save as run.png. $FRAME A RUN cycle, 8 frames in a 4x2 grid, facing right, body leaning forward, legs stretching and gathering."
run wave "$COMMON $SHEET Save as wave.png. $FRAME A WAVE: 6 frames in a 3x2 grid, sitting upright facing right, one front paw raised waving to the viewer back and forth twice."
run jump "$COMMON $SHEET Save as jump.png. $FRAME A JUMP, 8 frames in a 4x2 grid: crouch, take off, peak, fall, landing squash."
run sit "$COMMON $SHEET Save as sit.png. $FRAME A SIT: 6 frames in a 3x2 grid facing right: first frame standing, then settling down to sit, then sitting with a small tail or head wiggle and one blink."
run lookback "$COMMON $SHEET Save as lookback.png. $FRAME A LOOK-BACK: 6 frames in a 3x2 grid, walking right, then turning the head to look back toward the viewer, then turning forward again."
echo finished > ALL.done
