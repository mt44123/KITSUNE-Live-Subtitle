# 🦊 KITSUNE Subtitle

**Real-time multilingual subtitles for live streams, videos, and voice chat.**

KITSUNE Subtitle is a desktop application that provides fast, local-first, AI-powered subtitles and translation for live content.

Designed for stream viewers, esports fans, content creators, and multilingual communities.

> **Status:** 🚧 Early Development (Pre-Alpha)

---

## ✨ Vision

Watching a stream in another language should be as easy as turning on subtitles.

KITSUNE Subtitle aims to remove language barriers by providing:

- 🎙️ Real-time speech recognition
- 🌍 Real-time translation
- 💬 Live chat translation
- 🖥️ Local-first processing
- 🔌 Extensible plugin architecture

---

## 🚀 Planned Features

### Real-Time Subtitles

- Low-latency speech recognition
- GPU acceleration (NVIDIA RTX)
- Multiple speech engines
- Configurable subtitle overlay

### Translation

- Automatic language detection
- Multiple translation providers
- Real-time subtitle translation
- Custom translation profiles

### Chat Translation

- Twitch chat
- YouTube Live chat
- Future platform support

### Platform Integrations

- Twitch
- YouTube
- Discord
- OBS
- Local media files

### AI Features

- Stream summaries
- Timeline generation
- Searchable transcripts
- Highlight detection

---

## 🏗️ Architecture

KITSUNE Subtitle follows a layered architecture based on Clean Architecture principles.

```
UI
↓
Application
↓
Domain
↑
Plugins
↑
Infrastructure
```

For details see:

- `PROJECT_CONSTITUTION.md`
- `ARCHITECTURE.md`
- `AI_RULES.md`

---

## 🛠️ Technology Stack

### Desktop

- Tauri v2

### Frontend

- React
- TypeScript
- Vite
- Tailwind CSS

### Backend

- Rust

### Speech Recognition

- faster-whisper (planned)
- whisper.cpp (planned)

### Translation

Provider-based plugin architecture

### Storage

- SQLite
- JSON

---

## 📦 Repository Structure

```
apps/
packages/
plugins/
docs/
assets/
scripts/
tests/
```

---

## 📅 Roadmap

### Sprint 0

- Project foundation
- Architecture
- Development environment

### Sprint 1

- Audio Capture Engine

### Sprint 2

- Speech Recognition

### Sprint 3

- Subtitle Engine

### Sprint 4

- Translation Engine

### Sprint 5

- Overlay UI

---

## 🤝 Contributing

Contributions are welcome after the first public alpha.

Before contributing, please read:

- `PROJECT_CONSTITUTION.md`
- `ARCHITECTURE.md`
- `AI_RULES.md`

---

## 📄 License

License information will be added before the first public release.

---

## 🌐 Related Projects

### OW KITSUNE GUIDE

An Overwatch esports platform for tracking professional players, live streams, videos, and more.

---

## ❤️ Philosophy

KITSUNE Subtitle is built with three priorities:

1. Local First
2. Privacy First
3. User First

Fast software.
Simple software.
Software that lasts.

---

## 🚧 Project Status

This project is currently in active development.

The first milestone focuses on building a solid foundation before implementing subtitle functionality.
