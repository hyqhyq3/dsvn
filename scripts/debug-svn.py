#!/usr/bin/env python3
"""Debug SVN client requests by simulating what SVN does"""

import http.client
import xml.etree.ElementTree as ET

def make_request(host, port, path, method, headers, body=None):
    """Make HTTP request"""
    conn = http.client.HTTPConnection(host, port)
    conn.putrequest(method, path)

    for header, value in headers.items():
        conn.putheader(header, value)

    conn.endheaders()

    if body:
        conn.send(body.encode('utf-8'))

    response = conn.getresponse()
    data = response.read()
    conn.close()

    return response.status, response.headers, data.decode('utf-8', errors='replace')

def test_options():
    """Test OPTIONS request"""
    print("Testing OPTIONS /svn...")
    status, headers, body = make_request(
        'localhost', 8080, '/svn', 'OPTIONS',
        {'User-Agent': 'SVN/1.14.0'}
    )
    print(f"Status: {status}")
    print(f"Headers: {dict(headers)}")
    print(f"Body: {body[:200]}")
    print()

def test_propfind_root():
    """Test PROPFIND on /svn"""
    print("Testing PROPFIND /svn...")
    status, headers, body = make_request(
        'localhost', 8080, '/svn', 'PROPFIND',
        {
            'Depth': '0',
            'Content-Type': 'text/xml',
            'User-Agent': 'SVN/1.14.0'
        },
        '''<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop>
    <resourcetype/>
    <version-controlled-configuration/>
  </prop>
</propfind>'''
    )
    print(f"Status: {status}")
    print(f"Body:\n{body}")
    print()

def test_propfind_collection():
    """Test PROPFIND on /svn/ with Depth: 1"""
    print("Testing PROPFIND /svn/ with Depth: 1...")
    status, headers, body = make_request(
        'localhost', 8080, '/svn/', 'PROPFIND',
        {
            'Depth': '1',
            'Content-Type': 'text/xml',
            'User-Agent': 'SVN/1.14.0'
        },
        '''<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop>
    <resourcetype/>
    <baseline-collection/>
    <version-controlled-configuration/>
  </prop>
</propfind>'''
    )
    print(f"Status: {status}")
    print(f"Body:\n{body}")
    print()

def test_vcc():
    """Test version-controlled-configuration"""
    print("Testing GET /svn/!svn/vcc/default...")
    status, headers, body = make_request(
        'localhost', 8080, '/svn/!svn/vcc/default', 'GET',
        {'User-Agent': 'SVN/1.14.0'}
    )
    print(f"Status: {status}")
    print(f"Headers: {dict(headers)}")
    print()

if __name__ == '__main__':
    print("=" * 60)
    print("DSvn Server Debug - SVN Protocol Simulation")
    print("=" * 60)
    print()

    try:
        test_options()
        test_propfind_root()
        test_propfind_collection()
        test_vcc()
        print("=" * 60)
        print("All tests completed successfully!")
        print("=" * 60)
    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
