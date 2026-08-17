# Evidence Directory

Generated verification artifacts go under `_evidence/latest/`.

Expected files include:

- `serial.log`
- `host-tests.log`
- `build.log`
- screenshots when visual boot checks are added

Generated files in `_evidence/latest/` are disposable and may be recreated by `make test-boot` or `make test-host`.
