#!/bin/sh
# Builds the synthetic audio files jukebox-audio's tests decode. Each is the same two
# seconds: a quarter second of silence, then a 1 kHz tone, so the tests can tell exactly
# where audio starts — which is how encoder delay that isn't trimmed shows up.
#
# Needs ffmpeg. The HE-AAC file uses Apple's AAC encoder, so run this on macOS.
#
#   sh crates/jukebox-audio/scripts/make-fixtures.sh
set -eu
out="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures"
rm -rf "$out" && mkdir -p "$out"
f() { ffmpeg -v error -y "$@"; }

signal="aevalsrc='if(gte(t,0.25),0.5*sin(2*PI*1000*t),0)':s=44100:d=2:c=stereo"
f -f lavfi -i "$signal" -c:a pcm_s16le "$out/reference.wav"
f -i "$out/reference.wav" -c:a flac "$out/tone.flac"
f -i "$out/reference.wav" -c:a libmp3lame -b:a 96k "$out/tone.mp3"
f -i "$out/reference.wav" -c:a aac -b:a 64k "$out/tone.m4a"
f -i "$out/reference.wav" -c:a aac_at -b:a 64k "$out/tone-apple.m4a"
f -i "$out/reference.wav" -c:a libopus -b:a 48k "$out/tone.opus"
f -i "$out/reference.wav" -c:a vorbis -strict -2 "$out/tone.ogg"
# 48 kHz input, to exercise resampling to a 44.1 kHz output and back.
f -f lavfi -i "aevalsrc='if(gte(t,0.25),0.5*sin(2*PI*1000*t),0)':s=48000:d=2:c=stereo" -c:a flac "$out/tone-48k.flac"
# HE-AAC: SBR signalled implicitly, which symphonia decodes at half bandwidth.
f -i "$out/reference.wav" -c:a aac_at -profile:a 4 -b:a 32k "$out/he-aac.m4a"
# 5.1 AAC, which symphonia refuses.
f -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=1" -filter_complex "[0]asplit=6[a][b][c][d][e][g];[a][b][c][d][e][g]join=inputs=6:channel_layout=5.1" -c:a aac -b:a 128k "$out/surround.m4a"
printf 'this is not audio\n' > "$out/not-audio.mp3"
ls -la "$out"
