# Offline TLS fixtures

These DER certificates and the PKCS#8 server key are disposable test material,
generated with Python cryptography using ECDSA P-256 and SHA-256. They are not
production credentials. The signing CA's private key was discarded.

`root.der` is a private CA valid from 2020-01-01 through 2100-01-01.
`server.der` is signed by that CA, has the DNS name `localhost`, and has the
same validity period. `expired.der` uses the same server key and DNS name but
expired on 2021-01-01. No certificate includes a loopback IP address.

The transport tests perform real local TLS handshakes. Production Mozilla
roots reject this CA. Test-only trust permits a positive handshake on TLS 1.2
and 1.3, while hostname mismatch and expiry still fail.
