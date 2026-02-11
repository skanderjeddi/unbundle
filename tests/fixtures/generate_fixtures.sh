#!/usr/bin/env bash
# Generate small test fixture media files using FFmpeg CLI.
#
# Run this script once before running the integration tests:
#   bash tests/fixtures/generate_fixtures.sh
#
# Requirements: ffmpeg must be installed and on PATH.

set -euo pipefail

FIXTURES_DIR="$(cd "$(dirname "$0")" && pwd)"
echo "Generating test fixtures in ${FIXTURES_DIR}..."

# 1. sample_video.mp4 — 5 seconds, 640x480, 30 fps, with audio
#    Video: colour bars with a frame counter burned in
#    Audio: 1 kHz sine wave, stereo, 44100 Hz
ffmpeg -y \
    -f lavfi -i "testsrc=duration=5:size=640x480:rate=30" \
    -f lavfi -i "sine=frequency=1000:duration=5:sample_rate=44100" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
    -c:a aac -b:a 128k -ac 2 \
    -shortest \
    "${FIXTURES_DIR}/sample_video.mp4"
echo "  Created sample_video.mp4"

# 2. sample_audio_only.mp4 — 5 seconds, audio only (no video stream)
ffmpeg -y \
    -f lavfi -i "sine=frequency=440:duration=5:sample_rate=44100" \
    -c:a aac -b:a 128k -ac 2 \
    "${FIXTURES_DIR}/sample_audio_only.mp4"
echo "  Created sample_audio_only.mp4"

# 3. sample_video_only.mp4 — 5 seconds, video only (no audio stream)
ffmpeg -y \
    -f lavfi -i "testsrc=duration=5:size=320x240:rate=24" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
    -an \
    "${FIXTURES_DIR}/sample_video_only.mp4"
echo "  Created sample_video_only.mp4"

# 4. sample_short.mp4 — very short (0.5 seconds)
ffmpeg -y \
    -f lavfi -i "testsrc=duration=0.5:size=160x120:rate=10" \
    -f lavfi -i "sine=frequency=880:duration=0.5:sample_rate=44100" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
    -c:a aac -b:a 64k -ac 1 \
    -shortest \
    "${FIXTURES_DIR}/sample_short.mp4"
echo "  Created sample_short.mp4"

# 5. sample_video.mkv — MKV container (same content as sample_video.mp4)
ffmpeg -y \
    -f lavfi -i "testsrc=duration=3:size=320x240:rate=25" \
    -f lavfi -i "sine=frequency=500:duration=3:sample_rate=44100" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
    -c:a libvorbis -b:a 64k -ac 2 \
    -shortest \
    "${FIXTURES_DIR}/sample_video.mkv"
echo "  Created sample_video.mkv"

# 6. sample_with_subtitles.mkv — MKV with embedded SRT subtitle track
cat > "${FIXTURES_DIR}/temp_subs.srt" <<'EOF'
1
00:00:00,500 --> 00:00:02,000
Hello, world!

2
00:00:02,500 --> 00:00:04,000
This is a subtitle test.

3
00:00:04,500 --> 00:00:05,000
Goodbye!
EOF

ffmpeg -y \
    -f lavfi -i "testsrc=duration=5:size=320x240:rate=25" \
    -f lavfi -i "sine=frequency=500:duration=5:sample_rate=44100" \
    -i "${FIXTURES_DIR}/temp_subs.srt" \
    -c:v libx264 -preset ultrafast -pix_fmt yuv420p \
    -c:a aac -b:a 64k -ac 2 \
    -c:s srt \
    -shortest \
    "${FIXTURES_DIR}/sample_with_subtitles.mkv"
rm -f "${FIXTURES_DIR}/temp_subs.srt"
echo "  Created sample_with_subtitles.mkv"

echo ""
echo "All fixtures generated successfully."
echo "You can now run: cargo test"
