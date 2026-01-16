# Ausha

A real time audio transportation mechanism

# MindMap

# Dependencies

### Linux

Pulseaudio

### window

Wasapi

### Bash

```bash
pactl list sources short
ffmpeg -f pulse -i "variable check before command sink" out.wav
ffmpeg -f wasapi -i "audio=default" out.wav
```
