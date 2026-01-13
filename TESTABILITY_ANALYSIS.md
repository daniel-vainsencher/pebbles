# Pebbles Testability Analysis

## Executive Summary

Pebbles currently has limited testability due to tight coupling between Operations, Storage, and JJ components. While a test mode exists (PEBBLES_DB_PATH + .no_jj), the architecture makes it challenging to unit test business logic without I/O effects. This document analyzes the current state and provides prioritized recommendations for improvement.

## Current Architecture Analysis

### Component Structure

```
Operations (src/operations.rs)
  ├── Storage (src/storage.rs)  - Concrete type
  └── JJ (src/jj.rs)            - Concrete type
```

### Current Test Mode (uc-fddb76e9)

**Implementation:**
- `PEBBLES_DB_PATH` environment variable allows custom directory
- `.no_jj` marker file skips jj operations
- Enables integration testing with fake data

**Limitations:**
- Still requires file system access
- Still requires creating directories and JSON files
- Not suitable for fast unit tests
- Hard to test error conditions and edge cases

## Testability Challenges

### 1. No Abstraction Layer

**Problem:** Operations directly instantiates concrete types
```rust
pub struct Operations {
    storage: Storage,    // Concrete, not trait
    jj: JJ,              // Concrete, not trait
}
```

**Impact:**
- Cannot mock Storage for testing logic without file I/O
- Cannot mock JJ to test behavior without running commands
- Any test affects the file system

### 2. Mixed Concerns

**Problem:** Business logic and I/O intertwined in Operations methods

**Example - `update_status` method (operations.rs:93-114):**
```rust
pub fn update_status(&self, id: &str, new_status: Status) -> Result<()> {
    // I/O: Load from storage
    let uuid = self.resolve_id(id)?;
    let mut issue = self.storage.load_issue(&uuid)?;
    
    // Business logic: Status transition validation
    let old_status = issue.status.clone();
    issue.status = new_status;
    issue.update();
    
    // I/O: Save to storage and commit to jj
    let mut index = self.storage.load_index()?;
    self.storage.save_issue(&issue, &mut index)?;
    self.storage.save_index(&index)?;
    self.jj.commit(...)?;
    Ok(())
}
```

**Impact:**
- Cannot test validation logic independently
- Cannot test id resolution separately
- Cannot test state transitions without file system

### 3. Pure Functions Buried in Methods

**Problem:** Logic that could be pure functions is embedded in I/O-heavy methods

**Examples:

- `resolve_id` (operations.rs:388-409) - ID prefix matching logic
- `next` (operations.rs:241-273) - Dependency graph traversal
- `summary` (operations.rs:321-376) - Aggregation and statistics

**Impact:**
- Complex algorithms only testable with full file system setup
- Hard to test edge cases (empty lists, circular dependencies, malformed data)

### 4. Tight Coupling in Models

**Problem:** Some business logic embedded in models could be extracted

**Example - `Issue::add_relationship` (models.rs:188-207):**
```rust
pub fn add_relationship(&mut self, relation_type: RelationType, target_id: Uuid) -> Result<()>
```

**Impact:**
- Validation rules hard to test without full Issue construction
- Cannot test relationship constraints independently

## Recommended Improvements

### Priority 1: Introduce Traits for Storage and JJ

**Changes:**

```rust
// src/storage.rs - Add trait
pub trait StorageBackend {
    fn load_index(&self) -> Result<Index>;
    fn save_index(&self, index: &Index) -> Result<>;
    fn load_issue(&self, id: &Uuid) -> Result<Issue>;
    fn load_all_issues(&self) -> Result<Vec<Issue>>;
    fn save_issue(&self, issue: &Issue, index: &mut Index) -> Result<()>;
    fn delete_issue(&self, id: &Uuid, index: &Index) -> Result<()>;
    fn exists(&self) -> bool;
    fn init(&self) -> Result<()>;
}

// Make Storage implement the trait
impl StorageBackend for Storage { ... }

// src/jj.rs - Add trait
pub trait VersionControl {
    fn is_repo(&self) -> bool;
    fn init(&self) -> Result<()>;
    fn commit(&self, message: &str, file_path: &Path) -> Result<()>;
}

// Make JJ implement the trait
impl VersionControl for JJ { ... }

// Update Operations to use traits
pub struct Operations<S: StorageBackend, V: VersionControl> {
    storage: S,
    jj: V,
}
```

**Benefits:**
- Mock implementations for unit tests
- Test without file system
- Test without running jj commands
- Can inject fakes for specific scenarios

**Mock Implementation Example:**
```rust
pub struct FakeStorage {
    issues: HashMap<Uuid, Issue>,
    index: Index,
}

impl StorageBackend for FakeStorage {
    fn load_issue(&self, id: &Uuid) -> Result<Issue> {
        self.issues.get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Issue not found"))
    }
    
    fn save_issue(&self, issue: &Issue, index: &mut Index) -> Result<()> {
        // Implementation that just updates in-memory state
        Ok(())
    }
    // ... rest of trait methods
}
```

**Migration Path:**
1. Keep concrete `Operations` type as type alias: `type RealOperations = Operations<Storage, JJ>`
2. Update existing code to use `RealOperations`
3. Add trait-abased version for new code and tests
4. Gradually migrate tests to use mocked dependencies

### Priority 2: Extract Pure Business Logic

**Changes:**

Create `src/logic.rs` with pure functions:

```rust
// ID resolution - extracted from Operations::resolve_id
pub fn resolve_id(id_str: &str, known_ids: Vec<Uuid>) -> Result<Uuid> {
    if let Ok(uuid) = Uuid::parse_str(id_str) {
        if known_ids.contains(&uuid) {
            return Ok(uuid);
        }
        bail!("Issue with ID {} not found", uuid);
    }

    let matches: Vec<_> = known_ids
        .iter()
        .filter(|uuid| uuid.to_string().starts_with(id_str))
        .collect();

    match matches.len() {
        0 => bail!("No issue found with ID starting with '{}'", id_str),
        1 => Ok(*matches[0]),
        _ => bail!("Ambiguous ID '{}'. Multiple issues match.", id_str),
    }
}

// Dependency checking - extracted from Operations::next
pub fn is_unblocked(issue: &Issue, all_issues: &HashMap<Uuid, Issue>) -> bool {
    for rel in &issue.relationships {
        if rel.relation_type == RelationType::RequiredFor {
            if let Some(required) = all_issues.get(&rel.target_id) {
                if required.status != Status::Done {
                    return false;
                }
            }
        }
    }
    true
}

// Epic children - extracted from Operations::epic
pub fn get_epic_children(epic_id: &Uuid, all_issues: &Vec<Issue>) -> Vec<Issue> {
    all_issues
        .iter()
        .filter(|issue| {
            issue.relationships.iter().any(|rel| {
                rel.relation_type == RelationType::PartOf && rel.target_id == *epic_id
            })
        })
        .cloned()
        .collect()
}

// Statistics - extracted from Operations::summary
pub fn calculate_summary(all_issues: &Vec<Issue>) -> ProjectSummary {
    let mut by_status: HashMap<Status, usize> = HashMap::new();
    let mut by_type: HashMap<IssueType, usize> = HashMap::new();
    
    for issue in all_issues {
        *by_status.entry(issue.status.clone()).or_insert(0) += 1;
        *by_type.entry(issue.issue_type.clone()).or_insert(0) += 1;
    }
    
    // ... gather epic info and count actionable
    
    ProjectSummary {
        total_issues: all_issues.len(),
        by_status,
        by_type,
        // ...
    }
}
```

**Benefits:**
- All logic testable with simple data structures
- No I/O required for tests
- Easy to test edge cases
- Reusable across different operations

**Test Example:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_id_complete_uuid() {
        let uuid = Uuid::new_v4();
        let known_ids = vec![uuid];
        assert_eq!(resolve_id(&uuid.to_string(), known_ids).unwrap(), uuid);
    }

    #[test]
    fn test_resolve_id_ambiguous() {
        let uuid1 = Uuid::parse_str("aaaa0000-0000-0000-0000-000000000000").unwrap();
        let uuid2 = Uuid::parse_str("aaaa1111-0000-0000-0000-000000000000").unwrap();
        let known_ids = vec![uuid1, uuid2];
        
        let result = resolve_id("aaaa", known_ids);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Ambiguous"));
    }

    #[test]
    fn test_is_unblocked_no_dependencies() {
        let issue = Issue::new("Test".to_string(), None, IssueType::Bug);
        let all_issues = HashMap::new();
        assert!(is_unblocked(&issue, &all_issues));
    }

    #[test]
    fn test_is_unblocked_with_blocking_issue() {
        let mut blocker = Issue::new("Blocker".to_string(), None, IssueType::Bug);
        blocker.status = Status::Todo;
        
        let mut waiting = Issue::new("Waiting".to_string(), None, IssueType::Bug);
        waiting.relationships.push(Relationship {
            relation_type: RelationType::RequiredFor,
            target_id: blocker.id,
        });

        let mut all_issues = HashMap::new();
        all_issues.insert(blocker.id, blocker);
        all_issues.insert(waiting.id, waiting.clone());
        
        assert!(!is_unblocked(&waiting, &all_issues));
    }
}
```

### Priority 3: Separate Command Execution from Logic

**Changes:**

Refactor Operations methods to separate concerns:

```rust
// Current: update_status mixes everything
pub fn update_status(&self, id: &str, new_status: Status) -> Result<()> { ... }

// Split into pure function and runner
pub fn validate_status_change(issue: &Issue, new_status: Status) -> Result<()> {
    // Pure validation logic
    // - Check if status transition is valid
    // - Check invariants
    Ok(())
}

pub fn execute_status_change<S: StorageBackend, V: VersionControl>(
    operations: &Operations<S, V>,
    uuid: Uuid,
    new_status: Status
) -> Result<()> {
    // I/O orchestration only
    let mut issue = operations.storage.load_issue(&uuid)?;
    validate_status_change(&issue, new_status)?;
    issue.status = new_status;
    issue.update();
    // ... save and commit
    Ok(())
}
```

**Benefits:**
- Validation logic testable without I/O
- Clear separation of concerns
- Easier to understand and maintain

### Priority 4: Add Helper Test Factory

**Changes:**

```rust
// test_utils.rs
pub struct IssueBuilder {
    title: String,
    issue_type: IssueType,
    status: Status,
    description: Option<String>,
    relationships: Vec<Relationship>,
}

impl IssueBuilder {
    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }
    
    pub fn bug() -> Self {
        Self::new(IssueType::Bug)
    }
    
    pub fn epic() -> Self {
        Self::new(IssueType::Epic)
    }
    
    pub fn with_relationship(mut self, rel_type: RelationType, target: Uuid) -> Self {
        self.relationships.push(Relationship {
            relation_type: rel_type,
            target_id: target,
        });
        self
    }
    
    pub fn build(self) -> Issue {
        Issue::new(self.title, self.description, self.issue_type)
    }
}
```

**Benefits:**
- Easier test setup
- More readable tests
- Consistent test data

## Implementation Priorities

### Phase 1: Low Risk, High Value (Do First)

1. **Extract pure functions to src/logic.rs**
   - `resolve_id`, `is_unblocked`, `get_epic_children`, `calculate_summary`
   - Add doctests demonstrating usage
   - Add unit tests covering edge cases
   
   **Effort:** 2-3 hours  
   **Impact:** Immediate testability improvements for core algorithms  
   **Risk:** Low (pure refactoring, no behavior change)

2. **Add test utilities (IssueBuilder)**
   - Factory methods for creating test issues
   - Helper functions for common test scenarios
   
   **Effort:** 1-2 hours  
   **Impact:** Makes all tests easier to write  
   **Risk:** None (only affects test code)

### Phase 2: Medium Risk, Medium Value

3. **Create StorageBackend and VersionControl traits**
   - Define traits based on existing methods
   - Keep Storage and JJ as default implementations
   - Create FakeStorage and NoOpJJ mock implementations
   
   **Effort:** 4-6 hours  
   **Impact:** Enables mocking for integration tests  
   **Risk:** Medium (trait changes can break code, but maintain backward compatibility)

### Phase 3: Future Enhancement

4. **Update Operations to use trait parameters**
   - Make Operations generic over traits
   - Provide type aliases for convenience
   - Update all call sites
   
   **Effort:** 3-4 hours  
   **Impact:** Full testability with mocked dependencies  
   **Risk:** Medium (requires API changes to main.rs, cli.rs)

5. **Add comprehensive test suite**
   - Unit tests for all pure functions
   - Integration tests using fake backends
   - Property-based tests for edge cases
   
   **Effort:** 10-15 hours  
   **Impact:** High confidence in code correctness  
   **Risk:** None (test-only changes)

## Testing Strategy Recommendations

### Current Approach: Integration Tests with trycmd

**Example:** tests/workflows/core-workflow.trycmd

**Strengths:**
- Tests actual CLI behavior
- Documents expected output
- Catches regressions in command-line interface

**Limitations:**
- Slow (requires full setup/teardown)
- Hard to test error conditions
- Limited coverage of internal logic

### Recommended Approach: Layered Testing

```
┌─────────────────────────────────┐
│   CLI Integration Tests         │  trycmd - end-to-end workflows
│   (current approach)            │
└─────────────────────────────────┘
         ↓
┌─────────────────────────────────┐
│   Operations Tests              │  Use mocked Storage/JJ
│   (new - integration logic)     │
└─────────────────────────────────┘
         ↓
┌─────────────────────────────────┐
│   Logic Unit Tests              │  Pure functions, no I/O
│   (new - business logic)        │
└─────────────────────────────────┘
```

**Test Coverage Goals:**
- **Trycmd tests:** All CLI commands and major workflows
- **Operations tests:** Command orchestration with mocks
- **Unit tests:** 100% coverage of pure logic functions

## Conclusion

Pebbles has a solid foundation with the existing test mode (PEBBLES_DB_PATH), but can significantly improve testability by:

1. Extracting pure business logic functions (Priority 1)
2. Introducing trait abstractions for Storage and JJ (Priority 2)  
3. Separating validation from I/O (Priority 3)
4. Building comprehensive test utilities (Priority 4)

The phased approach allows gradual improvement with minimal risk. Start with extracting pure functions for immediate testability gains, then progress to trait-based abstractions for full mocking capabilities.

This will enable confident refactoring, faster development cycles, and catch bugs earlier in the development process.