#!/usr/bin/env bash
cd "$(dirname "$0")"
vlc --no-video-title-show --play-and-exit build/*.iso >/dev/null 2>&1
