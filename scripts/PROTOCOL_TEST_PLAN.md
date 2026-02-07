# DSvn Protocol Validation Test Plan
## TDD Approach for macOS ARM Segfault Workaround

### Problem Statement
- SVN client 1.14.3 on macOS ARM has a known segfault issue during checkout
- Server works correctly (verified with curl)
- Need a comprehensive protocol validation suite that doesn't rely on SVN client

### Goals
1. Create a curl-based protocol validation test suite
2. Test all WebDAV/SVN protocol methods
3. Provide a reliable alternative to SVN client tests
4. Enable CI/CD integration without SVN client dependency

### Test Categories

#### 1. Basic HTTP Methods
- [ ] OPTIONS - Server capability discovery
- [ ] HEAD - Header-only requests
- [ ] GET - File retrieval
- [ ] PUT - File creation/update
- [ ] DELETE - Resource removal

#### 2. WebDAV Collection Methods
- [ ] MKCOL - Directory creation
- [ ] PROPFIND - Property retrieval (Depth: 0, 1, infinity)
- [ ] PROPPATCH - Property modification

#### 3. DeltaV Versioning Methods
- [ ] CHECKOUT - Working resource creation
- [ ] CHECKIN - Committing changes
- [ ] MERGE - Integrating changes
- [ ] MKACTIVITY - Transaction creation

#### 4. SVN-Specific Methods
- [ ] REPORT - Log retrieval, update reports
- [ ] COPY - Resource copying
- [ ] MOVE - Resource moving
- [ ] LOCK/UNLOCK - Resource locking (stubs acceptable)

#### 5. Protocol Compliance Tests
- [ ] DAV headers present
- [ ] SVN headers present
- [ ] Correct Content-Type for XML responses
- [ ] Proper HTTP status codes
- [ ] XML response format validation

#### 6. End-to-End Workflow Tests
- [ ] Full commit workflow (MKACTIVITY → PUT → MERGE)
- [ ] Directory operations (MKCOL → PUT → DELETE)
- [ ] Property workflow (PROPPATCH → PROPFIND)

### Success Criteria
- All tests pass without SVN client
- Test suite runs in < 30 seconds
- Clear PASS/FAIL output for each test
- Exit code 0 for success, non-zero for failure

### Implementation Plan

#### Phase 1: RED (Create Failing Tests)
1. Create protocol-validation.sh with test framework
2. Write tests that check expected behavior
3. Run tests and verify they fail (or pass if already implemented)

#### Phase 2: GREEN (Make Tests Pass)
1. Implement/fix any missing WebDAV handlers
2. Ensure all responses match expected format
3. Run tests until all pass

#### Phase 3: REFACTOR (Improve Quality)
1. Clean up test code
2. Add better error messages
3. Optimize test execution
4. Add documentation

### Test Script Structure
```
protocol-validation.sh
├── setup_server()      # Start test server
├── teardown_server()   # Cleanup
├── test_options()      # Test OPTIONS method
├── test_propfind()     # Test PROPFIND method
├── test_report()       # Test REPORT method
├── test_merge()        # Test MERGE method
├── test_get()          # Test GET method
├── test_put()          # Test PUT method
├── test_mkcol()        # Test MKCOL method
├── test_delete()       # Test DELETE method
├── test_checkout()     # Test CHECKOUT method
├── test_checkin()      # Test CHECKIN method
├── test_mkactivity()   # Test MKACTIVITY method
└── run_all_tests()     # Execute all tests
```

### Notes
- Use curl for all HTTP requests
- Parse XML responses with basic string matching
- Track test count and success rate
- Generate detailed failure messages
