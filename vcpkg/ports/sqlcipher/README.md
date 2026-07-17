# roomrs SQLCipher overlay port

이 overlay는 Microsoft vcpkg의 `sqlcipher` port에
`SQLITE_ENABLE_PREUPDATE_HOOK`만 추가합니다. roomrs의 `live` feature가
SQLCipher system backend에서도 preupdate API를 사용하기 때문입니다.

- upstream: `microsoft/vcpkg@545b5dc234f4015a6fb32bf6cba863bb6b41cb9c`
- upstream port: `sqlcipher` 4.6.1, port-version 3
- overlay port-version: 4
- 변경: amalgamation 생성과 최종 C compile 양쪽에
  `SQLITE_ENABLE_PREUPDATE_HOOK` 정의

`port-version` 증가가 compile option 변경을 vcpkg binary cache ABI key에
반영합니다. `portfile.cmake`는 upstream SQLCipher `LICENSE.md`를 package의
`share/sqlcipher/copyright`로 설치하며, `vcpkg.json`은 BSD-3-Clause를
명시합니다.
