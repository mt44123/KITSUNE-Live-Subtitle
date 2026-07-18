# KITSUNE Subtitle Architecture

Version: 1.0.0

Status: Active

---

# Overview

KITSUNE Subtitle is built using a layered architecture based on Clean Architecture principles.

The project is designed to be:

- Easy to understand
- Easy to maintain
- AI-friendly
- Plugin-oriented
- Cross-platform ready

The Core never depends on external platforms.

External systems always depend on the Core.

---

# High-Level Architecture

```
+-----------------------------------------------------------+
|                           UI                              |
|             React + TypeScript + Tailwind                 |
+-----------------------------▲-----------------------------+
                              |
+-----------------------------|-----------------------------+
|                  Application Layer                        |
|      Commands / Use Cases / State Management              |
+-----------------------------▲-----------------------------+
                              |
+-----------------------------|-----------------------------+
|                     Domain (Core)                         |
|    Subtitle • Translation • Audio • Profiles             |
+-----------------------------▲-----------------------------+
                              |
+-----------------------------|-----------------------------+
|                     Plugin Layer                          |
|  Twitch • YouTube • OBS • DeepL • OpenAI • Discord        |
+-----------------------------▲-----------------------------+
                              |
+-----------------------------|-----------------------------+
|                  Infrastructure Layer                     |
| WASAPI • SQLite • Files • Network • OS APIs              |
+-----------------------------------------------------------+
```

---

# Layer Responsibilities

## UI

Responsible for presentation only.

Responsibilities:

- Display subtitles
- Display chat
- Display translation
- Display settings
- User interaction

The UI never contains business logic.

---

## Application Layer

Coordinates all user actions.

Responsibilities:

- Start subtitle session
- Stop subtitle session
- Load settings
- Save settings
- Plugin lifecycle
- Profile loading

Contains application workflows.

---

## Domain Layer

The heart of the application.

Contains no UI.

Contains no platform-specific code.

Contains no third-party SDKs.

Responsible for:

- Subtitle models
- Translation models
- Audio pipeline
- Chat pipeline
- Business rules

Everything here should be testable.

---

## Plugin Layer

Provides optional functionality.

Examples:

- Twitch
- YouTube
- Discord
- OBS
- DeepL
- Google Translate
- OpenAI
- Future providers

Plugins communicate only through public interfaces.

Plugins never access internal application code directly.

---

## Infrastructure Layer

Responsible for platform-specific implementation.

Examples:

- Windows Audio (WASAPI)
- File System
- SQLite
- Networking
- Auto Updater
- Logging

Infrastructure implements interfaces defined by the Domain.

---

# Dependency Rule

Dependencies always point inward.

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

The Domain must never depend on:

- React
- Tauri
- Rust crates
- Windows APIs
- Whisper
- DeepL
- OpenAI

Only interfaces.

---

# Core Modules

```
Core

├── Audio
├── Subtitle
├── Translation
├── Chat
├── Profiles
├── Settings
├── Plugin
└── Shared
```

Every module is independent.

Communication occurs through interfaces.

---

# Audio Pipeline

```
Windows Audio

↓

Audio Engine

↓

Speech Engine

↓

Subtitle Engine

↓

Translation Engine

↓

UI
```

Each stage is replaceable.

---

# Chat Pipeline

```
Provider

↓

Chat Parser

↓

Translation

↓

UI
```

Providers include:

- Twitch
- YouTube
- Discord

---

# Translation Pipeline

```
Original Text

↓

Translation Provider

↓

Translated Text

↓

UI
```

Translation providers are plugins.

---

# Speech Pipeline

```
Audio

↓

Speech Provider

↓

Transcript

↓

Subtitle

↓

Translation

↓

UI
```

Speech providers are replaceable.

Examples:

- faster-whisper
- whisper.cpp
- Future AI models

---

# Plugin System

Plugins must:

- Be isolated
- Be replaceable
- Be independently updateable

Plugins may provide:

- Translation
- Chat
- AI
- Platform integrations
- OCR
- Exporters

Plugins must never depend on each other.

---

# Profiles

Profiles customize behavior.

Example:

```
Kevster

Language:
Swedish

Translation:
Japanese

Chat:
Japanese
```

Profiles can be:

- Player profiles
- User profiles
- Workspace profiles

---

# Data Storage

Configuration

```
JSON
```

Application Data

```
SQLite
```

Logs

```
Text
```

Exports

```
TXT
SRT
JSON
```

---

# AI Development Principles

The project is optimized for AI-assisted development.

Requirements:

- Small files
- Clear names
- One responsibility per module
- Well documented interfaces
- Minimal hidden behavior

Architecture should be understandable by humans and AI.

---

# Future Expansion

The architecture should support future features without redesign.

Examples:

- OCR subtitles
- Video files
- Voice chat
- AI summaries
- AI highlights
- Clip generation
- Plugin marketplace

---

# Final Principle

If a new feature requires changing multiple unrelated modules,
the architecture should be reconsidered.

Good architecture makes new features feel easy.
