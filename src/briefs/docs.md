---
id: docs
name: Docs
---
# What it does
Writes or updates documentation as Markdown files in a project's folder: guides, READMEs, reference pages, notes. It
reads the code the docs describe but doesn't change code, and it doesn't publish pages anywhere else.

# Steps
1. outliner: Reads the task, the existing docs and the code they describe, and proposes what to write where.
2. tech-writer: Writes or updates the Markdown files the outline names, in the task's folder.
3. editor: Checks the files for accuracy against the code and the task, clarity, and the project's style. May send
   the work back to step 2, at most twice.

# Checks
- the tests pass
- the editor read the change
- nothing others see without your yes

# Asks you
Before anything others can see, and when it is stuck.
