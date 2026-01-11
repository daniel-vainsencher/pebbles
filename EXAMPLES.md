# Pebbles Usage Examples

## Getting Started

### Starting a New Project

```bash
# Create a new project directory
mkdir my-project
cd my-project

# Initialize pebbles
pebbles init
```

### Managing a Sprint

```bash
# Create sprint tasks
pebbles new "Setup project structure"
pebbles new "Implement authentication" -d "JWT-based auth with refresh tokens"
pebbles new "Add database migrations"
pebbles new "Write API documentation"
pebbles new "Deploy to staging"

# View all tasks
pebbles list
```

Output:
```
a1b2c3d4 [TODO] Deploy to staging
e5f6g7h8 [TODO] Write API documentation
i9j0k1l2 [TODO] Add database migrations
m3n4o5p6 [TODO] Implement authentication
q7r8s9t0 [TODO] Setup project structure
```

### Working Through Tasks

```bash
# Start working on authentication
pebbles status m3n4 in_progress

# Check what's in progress
pebbles list --status in_progress
```

Output:
```
m3n4o5p6 [IN PROGRESS] Implement authentication
```

```bash
# Complete it
pebbles status m3n4 done

# Start next task
pebbles status q7r8 in_progress
```

### Daily Standup View

```bash
# What's done?
pebbles list --status done

# What's in progress?
pebbles list --status in_progress

# What's still todo?
pebbles list --status todo
```

## Advanced Workflows

### Bug Triage

```bash
# Create bugs as you find them
pebbles new "Fix null pointer in user service"
pebbles new "Dashboard shows wrong date format"
pebbles new "Memory leak in background worker"

# Later, prioritize and work on them
pebbles list
pebbles show a1b2c  # Check details
pebbles status a1b2c in_progress  # Start working
```

### Feature Development

```bash
# Break down a feature into tasks
pebbles new "User Profile Feature: Database schema" -d "Add users table with profile fields"
pebbles new "User Profile Feature: API endpoints" -d "GET/PUT /api/users/:id/profile"
pebbles new "User Profile Feature: Frontend UI" -d "Profile page with edit form"
pebbles new "User Profile Feature: Tests" -d "Unit and integration tests"

# Work through them in order
pebbles list
```

### Code Review Checklist

```bash
# Create a checklist for code reviews
pebbles new "Code Review: Check error handling"
pebbles new "Code Review: Verify tests pass"
pebbles new "Code Review: Check documentation"
pebbles new "Code Review: Review security implications"

# Mark off as you go
pebbles status <id> done
pebbles status <id> done
```

### Refactoring Tracking

```bash
# Track refactoring tasks
pebbles new "Refactor: Extract payment processing module"
pebbles new "Refactor: Consolidate error handling"
pebbles new "Refactor: Remove deprecated API endpoints"

# Update as you work
pebbles status <id> in_progress
pebbles update <id> -d "Found 15 usages to update"
pebbles status <id> done
```

## Working with Short IDs

```bash
# Create an issue
pebbles new "Fix the bug"
# Output: ✓ Created issue 53ed9aa8

# Use just the first few characters
pebbles show 53ed
pebbles status 53ed in_progress
pebbles update 53ed --title "Fix the critical bug"
pebbles status 53ed done

# Even shorter if unique
pebbles show 53e
pebbles status 53e done
```

## Updating Issues

### Change Title

```bash
pebbles update a1b2c --title "New better title"
```

### Change Description

```bash
pebbles update a1b2c --description "Updated detailed description"
```

### Change Both

```bash
pebbles update a1b2c \
  --title "Completely rewritten title" \
  --description "And a new description too"
```

## Viewing Issue Details

```bash
# Full details
pebbles show a1b2c
```

Output:
```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
ID: a1b2c3d4-e5f6-7890-a1b2-c3d4e5f6g7h8
Title: Implement authentication
Status: IN PROGRESS
Description: JWT-based auth with refresh tokens
Created: 2026-01-02 10:30:45
Updated: 2026-01-02 14:22:13
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

## Cleaning Up

### Delete Completed Issues

```bash
# View done issues
pebbles list --status done

# Delete ones you don't need anymore
pebbles delete a1b2c
pebbles delete e5f6g
```

### Delete Obsolete Issues

```bash
# Maybe you decided not to do something
pebbles show x9y8z
pebbles delete x9y8z
```

## Version Control Integration

### Check Jujutsu History

```bash
# See what changes were made
jj log

# See issue changes in detail
jj show
```

### Synchronize Across Machines

```bash
# On machine A
pebbles new "Task created on machine A"
jj git push  # If using git remote

# On machine B
jj git fetch
jj new main  # Create new change on top of main

# Your issues are now synced!
pebbles list
```

## AI Agent Workflows

### Agent Planning Phase

```bash
# Agent creates tasks for what it needs to do
pebbles new "Analyze requirements"
pebbles new "Design data model"
pebbles new "Implement core logic"
pebbles new "Write tests"
pebbles new "Update documentation"

# Agent can query its task list
pebbles list --status todo
```

### Agent Execution Phase

```bash
# As agent works, it updates status
pebbles status <id> in_progress
# ... does work ...
pebbles status <id> done

# Move to next task
pebbles list --status todo | head -1  # Get next task
pebbles status <next-id> in_progress
```

### Agent Error Handling

```bash
# If agent encounters a blocker
pebbles new "BLOCKED: Need API key from user"
# Agent can pause and request human help
```

## Power User Tips

### Combine with Shell Tools

```bash
# Count issues by status
pebbles list --status todo | wc -l
pebbles list --status done | wc -l

# Search titles (using raw JSON)
grep -r "authentication" .pebbles/issues/

# Get oldest issue
ls -lt .pebbles/issues/*.json | tail -1
```

### Scripting

```bash
#!/bin/bash
# Script to create a standard sprint

pebbles new "Sprint Planning"
pebbles new "Development Work"
pebbles new "Code Review"
pebbles new "Testing"
pebbles new "Sprint Retrospective"

echo "Sprint tasks created!"
pebbles list
```

### Custom Filters

```bash
# Create aliases in your .bashrc
alias peb-todo='pebbles list --status todo'
alias peb-doing='pebbles list --status in_progress'
alias peb-done='pebbles list --status done'

# Usage
peb-todo
peb-doing
```

## Team Workflows

### Pull Request Template

```bash
# Before starting PR work
pebbles new "PR #123: Review frontend changes"
pebbles new "PR #123: Test edge cases"
pebbles new "PR #123: Update documentation"

# Track progress
pebbles list
```

### Meeting Notes

```bash
# During a meeting
pebbles new "Action item: Update deployment docs"
pebbles new "Action item: Schedule security review"
pebbles new "Action item: Investigate performance issue"

# After meeting
pebbles list --status todo  # See all action items
```

### Onboarding Checklist

```bash
# Create for new team members
pebbles new "Onboarding: Setup development environment"
pebbles new "Onboarding: Read architecture docs"
pebbles new "Onboarding: Complete security training"
pebbles new "Onboarding: Deploy test change"
pebbles new "Onboarding: Pair program with mentor"

# New team member works through them
pebbles list
```

## Troubleshooting

### Issue Not Found

```bash
$ pebbles show xyz
Error: No issue found with ID starting with 'xyz'

# Solution: Check the list
pebbles list
```

### Ambiguous ID

```bash
$ pebbles show a
Error: Ambiguous ID 'a'. Multiple issues match.

# Solution: Use more characters
pebbles show a1b
```

### Not Initialized

```bash
$ pebbles list
Error: Pebbles not initialized. Run 'pebbles init' first.

# Solution: Initialize
pebbles init
```

## Best Practices

### Good Issue Titles

✅ **Good:**
- "Fix login button not responding on mobile"
- "Add pagination to user list API"
- "Refactor database connection pooling"

❌ **Bad:**
- "Fix bug" (too vague)
- "Update stuff" (what stuff?)
- "todo" (not descriptive)

### When to Use Descriptions

Use descriptions for:
- Implementation details
- Acceptance criteria
- Links to related resources
- Technical notes

```bash
pebbles new "Implement caching layer" -d "Use Redis for session storage. TTL: 1 hour. Handle connection failures gracefully."
```

### Keeping Issues Manageable

- Break large tasks into smaller issues
- Delete completed issues you don't need for history
- Use descriptive titles
- Update status regularly

## Integration Examples

### Git Hooks (if colocated with git)

```bash
# .git/hooks/pre-push
#!/bin/bash
if pebbles list --status in_progress | grep -q .; then
    echo "Warning: You have issues in progress"
    pebbles list --status in_progress
fi
```

### CI/CD Integration

```bash
# In your CI script
if ! pebbles list --status done | grep -q "All tests passing"; then
    echo "Test issue not marked done!"
    exit 1
fi
```

## Conclusion

Pebbles is designed to be simple and flexible. These examples show common patterns, but feel free to adapt them to your workflow!
