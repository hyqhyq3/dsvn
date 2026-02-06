#!/bin/bash
# Test SVN dump/load functionality

set -e

echo "========================================"
echo "DSvn Dump/Load Test"
echo "========================================"
echo

# Create a simple SVN dump file
cat > /tmp/test.dump << 'EOF'
SVN-fs-dump-format-version: 3
UUID: 12345678-1234-1234-1234-123456789abc

Revision-number: 0
Prop-content-length: 10
Content-length: 10

PROPS-END

Revision-number: 1
Prop-content-length: 116
Content-length: 116

K 7
svn:log
V 11
Initial commit
K 10
svn:author
V 5
admin
K 8
svn:date
V 27
2024-01-06T00:00:00.000000Z
PROPS-END

Node-path: trunk
Node-kind: dir
Node-action: add
Prop-content-length: 10
Content-length: 10

PROPS-END

Node-path: branches
Node-kind: dir
Node-action: add
Prop-content-length: 10
Content-length: 10

PROPS-END

Node-path: tags
Node-kind: dir
Node-action: add
Prop-content-length: 10
Content-length: 10

PROPS-END

Revision-number: 2
Prop-content-length: 109
Content-length: 109

K 7
svn:log
V 12
Add README.md
K 10
svn:author
V 5
admin
K 8
svn:date
V 27
2024-01-06T01:00:00.000000Z
PROPS-END

Node-path: trunk/README.md
Node-kind: file
Node-action: add
Prop-content-length: 10
Content-length: 10

PROPS-END
Text-content-length: 15
Content-length: 15

Hello DSvn MVP!

EOF

echo "✅ Created test dump file: /tmp/test.dump"
echo

# Load dump file using dsvn-admin
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Loading dump file"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if ./target/release/dsvn-admin load --file /tmp/test.dump; then
    echo "✅ Load successful"
else
    echo "❌ Load failed"
    exit 1
fi
echo

echo "========================================"
echo "✅ Test completed!"
echo "========================================"
echo
echo "Note: Data is stored in memory for MVP."
echo "To test with real SVN dump file:"
echo "  svnadmin dump /path/to/svn/repo > repo.dump"
echo "  ./target/release/dsvn-admin load --file repo.dump"
