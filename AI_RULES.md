# AI Development Rules

Version: 1.0.0

Status: Active

---

# Purpose

This document defines how AI assistants should contribute to the KITSUNE Subtitle project.

The goal is consistency, maintainability, and long-term quality.

Architecture always takes priority over implementation speed.

---

# Core Philosophy

AI should behave like a senior software engineer, not a code generator.

Before implementing anything:

1. Understand the architecture.
2. Understand existing modules.
3. Reuse existing abstractions.
4. Avoid unnecessary complexity.
5. Prefer maintainability.

Never generate code without understanding context.

---

# Required Reading Order

Before making changes, AI should read:

1. PROJECT_CONSTITUTION.md
2. ARCHITECTURE.md
3. README.md
4. This document

Never skip these documents.

---

# General Rules

## Never Duplicate Logic

If functionality already exists,
reuse it.

Do not create similar implementations.

---

## Keep Files Small

Preferred:

100–300 lines

Maximum:

400 lines

If a file grows larger,
refactor.

---

## Single Responsibility

Each file should have one clear purpose.

Avoid "utility" files that become dumping grounds.

---

## Naming

Use descriptive names.

Good:

SubtitleSession

TranslationProvider

PluginManager

Bad:

Manager

Helper

Utils

Common

Misc

---

## Composition Over Inheritance

Prefer composing small modules.

Avoid deep inheritance.

---

## Interfaces First

Always depend on interfaces.

Concrete implementations belong in Infrastructure or Plugins.

---

## Dependency Direction

Dependencies always point toward the Domain.

Never the opposite.

---

# UI Rules

UI is presentation only.

Never place business logic inside React components.

React components should:

Display state

Receive user input

Call application services

Nothing more.

---

# State Management

Avoid global mutable state.

Prefer explicit state flow.

Keep state predictable.

---

# Error Handling

Never silently ignore errors.

Every error should:

Be logged

Provide meaningful information

Be recoverable whenever possible

---

# Plugin Rules

Plugins are isolated.

Plugins:

Cannot access application internals

Cannot depend on other plugins

Communicate only through Plugin APIs

Can be enabled or disabled independently

---

# Translation Providers

Translation engines are plugins.

The application should never depend on:

Google

DeepL

OpenAI

Directly.

Always use interfaces.

---

# Speech Providers

Speech recognition engines are replaceable.

Supported examples:

faster-whisper

whisper.cpp

Future models

Never hardcode implementations.

---

# Chat Providers

Chat providers are plugins.

Examples:

Twitch

YouTube

Discord

Future platforms

No provider-specific logic belongs in Core.

---

# Logging

Logs should be:

Readable

Structured

Useful

Avoid noisy logging.

---

# Performance

Real-time subtitles are latency-sensitive.

Always consider:

Memory usage

CPU usage

GPU usage

Rendering speed

Avoid premature optimization,
but never ignore performance.

---

# Documentation

Public APIs must be documented.

Complex modules should explain:

Purpose

Responsibilities

Usage

Design decisions

---

# Code Style

Prefer clarity.

Prefer explicit code.

Avoid clever tricks.

Future contributors should understand code quickly.

---

# AI Code Review Checklist

Before completing any task:

✓ Architecture respected

✓ No duplicated logic

✓ No unnecessary dependencies

✓ Small modules

✓ Clear names

✓ Proper interfaces

✓ Documentation updated

✓ Performance considered

---

# When Unsure

If multiple solutions are possible:

Choose the one that:

Is easier to understand

Is easier to maintain

Requires fewer dependencies

Can evolve without breaking existing code

---

# Final Principle

Generate code that a human would be happy to maintain five years from now.
