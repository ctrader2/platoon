---
id: coding
name: Coding
---
# What it does
Makes a change to code in a git repository: plans it, builds it in a worktree of its own, reviews it and runs the
tests, then takes it as far as the task says.

# Steps
1. planner: Reads the task and the code it touches and writes a short plan that restates the acceptance checks.
2. builder: Makes the change the plan describes in the task's folder and runs the tests.
3. reviewer: Reads the change against the task and the plan, and checks that each acceptance check could have failed
   before the change. May send the work back to step 2, at most twice.
4. verifier: Runs the tests and the build and reports plainly what passed and what failed. May send the work back to
   step 2, at most once.

# Checks
- the tests pass
- the reviewer read the change
- every acceptance box is ticked, with how
- nothing others see without your yes

# Asks you
Before anything others can see, and when it is stuck.
