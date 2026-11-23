# e-motional-lib

[English](README.md)

윈도우의 Edge motion을  Rust로 작성한 Linux 시스템 서비스로 비슷하게 구현한 프로젝트 입니다.
Windows 기반 laptop에는 기본적으로 적용되지만, linux에서도 사용하기 위해 만들어졌습니다.

터치패드 가장자리에 도달하면 포인터를 계속 움직입니다.

!서비스이며, 이름과 달리 재사용 가능한 라이브러리가 아닙니다!

선택한 evdev 장치를 독점 없이 read만 하며, uinput 장치를 통해 상대 포인터 이동만 추가하는 방식 입니다.

동작을 위한 최소 권한을 가진 root broker를 생성하며 input 그룹 가입, udev 권한 규칙은 필요하지 않습니다.

## 설치

요구 사항:

- **systemd 253 이상**과 systemd-logind가 실행 중인 Linux.
- **seat0**의 로컬 그래픽 X11 또는 Wayland 사용자 세션.
- uinput 커널 모듈/장치와 표준 `modprobe@.service` 유닛.
  기존 장치 권한으로 root가 선택한 터치패드를 읽고 `/dev/uinput`을 열 수 있어야 합니다.
- 소스 설치 시: 시스템 전체에서 사용할 수 있는 stable Rust/Cargo 툴체인, C 링커,
  pkg-config, libudev 및 libsystemd 개발 파일, curl, 표준 GNU 사용자 공간 도구.
  특정 사용자의 홈 디렉터리에만 설치된 Rust 툴체인은 격리된 빌더에서 의도적으로
  접근할 수 없습니다.

Arch Linux에서는 다음 명령으로 필수 패키지를 설치합니다.

```sh
sudo pacman -S --needed base-devel rust pkgconf systemd curl
```

설치 프로그램은 uinput 장치가 없으면 `uinput`을 로드합니다. 서비스 시작 시 브로커는
배포판의 표준 `modprobe@uinput.service` 유닛 실행을 요청합니다. 어느 과정도 모듈 로드
설정을 기록하거나 장치 권한, 계정, 그룹, ACL, udev 규칙을 변경하지 않습니다.
커널이나 보안 정책이 로드 또는 접근을 막으면 정책을 자동으로 바꾸지 않고 실패합니다.


다음 명령으로 설치 및 터치패드 보정 작업, 서비스 활성화를 합니다.
```sh
curl -fsSL https://raw.githubusercontent.com/DocRAID/e-motional-lib/main/install.sh | sudo sh
sudo systemctl enable --now e-motional-lib
```

첫 번째 명령은 파이프로 실행해도 대화형으로 동작합니다. 보정 입력은 `/dev/tty`에서
읽습니다. 사용할 터치패드를 선택하고, 터치패드의 상하좌우 가장자리 표면을 인식합니다.
Enter로 측정을 마치고 Edge-motion을 사용 할 X/Y 비율을
선택합니다.

### 설치 프로그램의 동작

1. root 권한, 필수 도구, 보호 대상 경로를 확인합니다. 관리 대상이 아닌 바이너리,
   수정된 관리 파일, 심볼릭 링크, 안전하지 않은 기존 설정 경로, 충돌하는 시스템
   유닛/드롭인, 충돌하는 정적 `e-motional-lib` 계정 또는 그룹이 있으면 거부합니다.
   관리 중인 설치의 DynamicUser를 중지한 뒤 NSS 사용자·그룹 정보를 확인합니다.
   권한을 자동으로 고치지 않습니다.
2. 업스트림 `main`을 하나의 GitHub 커밋으로 고정하고, 해당 리비전의 소스 압축 파일과
   유닛 세 개를 HTTPS로 내려받습니다. 미리 빌드된 릴리스 파일이 있다고 가정하지 않습니다.
3. 일회성 **DynamicUser** 시스템 서비스에서 압축을 풀고
   `cargo build --release --locked`로 컴파일합니다. Cargo 의존성, 빌드 스크립트,
   proc macro, 소스 압축 해제는 root로 실행하지 않습니다. 빌더는 전용 쓰기 가능
   작업 공간과 임시 디렉터리를 사용하며 홈/장치에는 접근할 수 없습니다.
   의존성 다운로드를 위한 네트워크 접근은 허용됩니다. 빌드 결과 바이너리만 root의
   준비 영역으로 복사하며, 빌드 유닛이 중지되면 작업 공간을 삭제합니다.
4. 바이너리와 시스템 유닛을 설치하고, 비공개 설치 매니페스트에 SHA-256 해시를
   기록합니다. 교체 inode는 기존 관리 파일의 소유권, 권한/ACL, 확장 속성을 보존합니다.
   관리 중인 설치를 업그레이드할 때는 교체/보정 전에 이 프로젝트의 시스템 유닛만
   중지하며, 완료 후에도 중지 상태로 둡니다.
5. systemd 유닛 정의를 다시 읽고 다음 명령을 실행합니다.

   ```sh
   /usr/bin/e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml < /dev/tty
   ```

   새 설정 디렉터리는 root 소유의 `0700`, 새 설정 파일은 root 소유의 `0600` 권한으로
   만듭니다. 원자적 교체 시 기존 소유권, 모드, POSIX ACL, SELinux/Smack 레이블을
   보존합니다. 보존할 수 없으면 정책을 완화하지 않고 저장을 중단합니다.
   기존 시스템 설정도 이미 이 비공개 권한과 root 소유권을 갖춰야 하며,
   심볼릭 링크/하드 링크가 아니어야 합니다. 그 외 경로는 거부합니다.
   설정을 취소하면 저장하지 않습니다. 기존 설정이 없으면 설치 프로그램은 실패를
   보고하며, 기존 설정이 있으면 취소해도 그대로 유지합니다.

설치 프로그램은 업그레이드 후를 포함해 **서비스를 활성화하거나 시작하지 않습니다**.
일부 파일을 복사한 뒤 실패해도 서비스는 중지 상태로 남습니다. 업그레이드가 중단되면
재시도 전에 해당 프로젝트 파일을 매니페스트와 수동으로 대조하고 정리해야 할 수 있습니다.
자신이 관리하는 설치임을 입증할 수 없으면 거부합니다. 이 설치 프로그램과 패키지
관리자를 통한 설치를 혼용하지 마세요.

curl 명령은 내려받은 설치 프로그램에 root 권한을 부여합니다. 먼저 검토하려면 파일로
내려받아 내용을 확인한 뒤 `sudo sh ./install.sh`로 실행하세요. HTTPS와 커밋 고정은
한 번의 설치에서 리비전 일관성을 유지하지만, 릴리스 서명이나 재현 가능한 빌드를
보장하지는 않습니다. 이 저장소, GitHub, 사용 중인 툴체인, 잠금 파일로 고정한 Cargo
의존성 소스를 신뢰해야 합니다. 설치 후 프로그램은 작은 root 브로커와 root 설정 코드를
실행합니다.

### 개발용 체크아웃

저장소: `git@github.com:DocRAID/e-motional-lib.git`.

```sh
git clone git@github.com:DocRAID/e-motional-lib.git
cd e-motional-lib
makepkg -si
sudo mkdir -m 700 /etc/e-motional-lib   # only if it does not already exist
sudo e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml
sudo systemctl enable --now e-motional-lib
```

```sh
systemctl --user disable --now e-motional-lib.service
# Only if you used the former project name:
systemctl --user disable --now libinput-edgemotion.service
```

마지막 명령은 이전 프로젝트 이름을 사용한 경우에만 실행합니다.

수동으로 설치한 이전 파일은 이 프로젝트의 파일임을 확실히 알 때만 삭제하세요.
패키지로 설치한 파일은 패키지 관리자로 처리합니다. 기존 사용자 설정은 이동하거나
변경하지 않습니다. 새 root 소유 시스템 설정에서 다시 보정해야 합니다.
이제 장치 항목은 선택적 대체 보정값이 아니라 보안 허용 목록입니다.

## 동작 방식

1. 소켓으로 활성화되는 **root 브로커**는 `/etc/e-motional-lib/config.toml`에 보정된
   식별 정보가 있는 터치패드만 엽니다. 장치 탐색에는 터치패드 태그, seat0 소속,
   키보드 기능이 없다는 조건이 필요합니다. 설정된 장치가 없으면 모든 장치를 관찰하는
   대신 오류를 냅니다. 실제 장치 노드를 변경하지 않고 핫플러그를 처리합니다.
2. 브로커는 접촉 프레임을 읽고, 크기가 제한되고 검증된 접촉 스냅샷 및 기하 정보/설정을
   비특권 워커에 보냅니다. evdev와 uinput 디스크립터는 브로커가 보유하며,
   워커에 전달하지 않습니다.
3. **DynamicUser 워커**는 가장자리 이동을 계산하고, 상속받은 단일 연결로 상대 이동을
   요청합니다. 브로커는 요청을 검증하고 제한한 뒤 가상 포인터를 통해
   `REL_X`, `REL_Y`, `SYN_REPORT`를 출력합니다. 실제 입력을 수정하거나 막지 않습니다.

손바닥으로 판정되지 않은 손가락이 정확히 하나여야 하며, 모든 가장자리 영역 바깥에서
접촉을 시작한 뒤 가장자리로 들어가 대기 시간(기본 150ms) 동안 머물러야 합니다.
클릭 여부는 활성화와 무관합니다. 클릭, 버튼 해제, 클릭한 채 드래그하는 동작 자체는
이동을 초기화하지 않습니다. 가장자리에서 시작한 접촉은 활성화되지 않습니다.
기본 가장자리 영역은 각 축의 양쪽 끝에서 각각 5%입니다. 가장자리 안쪽으로 더 깊이
들어갈수록 속도가 빨라지며, 모서리에서는 두 축이 함께 활성화될 수 있습니다.
안쪽 방향의 히스테리시스로 경계 부근에서 활성 상태가 반복해서 바뀌는 것을 방지합니다.

손가락을 떼거나, 두 번째 손가락이 닿거나, 활성 영역을 벗어나거나, 장치가 제거되거나,
입력이 유효하지 않거나 IPC가 오래되거나, 조건에 맞는 세션을 잃으면 추가 이동을 멈춥니다.
시작 시점, 입력 오버플로, 세션 전환 후에는 모든 손가락을 완전히 떼야 새 접촉으로
활성화할 수 있습니다. 실제 입력은 두 서비스 프로세스와 독립적으로 계속 동작합니다.

MT type A/type B와 지원되는 싱글터치 장치는 기존 접촉 추적 및 보정 로직을 사용합니다.
정체를 알 수 없는 접촉도 단일 손가락 조건을 판단할 때 접촉 수에 포함합니다.
측정된 축 해상도가 타당하면 사용하고, 그렇지 않으면 비례 방식의 대체 기하 계산을
사용하므로 밀리미터 기반 제한은 추정치입니다. 상대 이동에는 데스크톱 입력 스택의
가속/배율이 적용됩니다. 이동량이 화면 픽셀과 같거나 정해진 시간 안에 화면을
가로지른다고 보장하지 않습니다.

## 권한과 신뢰 경계

런타임의 유일한 특권 구성 요소는 장치 브로커입니다. root로 실행하되 capability 집합은
비어 있고, 새 권한 획득 금지, 읽기 전용 시스템 마운트, 홈 숨김, 전용 장치 네임스페이스,
장치 접근 제한, 주소 패밀리 제한, 시스템 호출/네임스페이스 강화가 적용됩니다.
`CAP_DAC_OVERRIDE`, 파일 capability, setuid, 사용자의 광범위한 `input` 그룹 가입이
아니라 기존 root 장치 DAC 권한에 의존합니다.

소켓 `/run/e-motional-lib/broker.sock`은 root 소유 `0700` 디렉터리 안에 있으며,
root 소유 `0600` 권한입니다. systemd가 `OpenFile=`로 root 권한으로 연결한 뒤 연결된
디스크립터를 워커에 전달합니다. 워커는 직접 해당 경로를 열 수 없습니다.
브로커는 연결 상대가 root인지 인증하고 워커는 브로커를 인증합니다. 소켓이 브로커를
독립적으로 활성화하므로 워커/브로커 시작 의존성 순환을 피합니다.
공개 IPC 서비스나 제한 없는 FD 브로커가 아닙니다.

| 프로세스 | 읽기/접근 | 쓰기/변경 |
| --- | --- | --- |
| DynamicUser 워커 | 실행 파일/공유 라이브러리, 전용 프로세스/장치 메타데이터, 상속받은 브로커 연결과 검증된 스냅샷. 호스트 설정, 홈, evdev, uinput, 사용자 세션 소켓에는 접근 불가 | 브로커 이동 메시지, journal 출력, 전용 임시 공간. 호스트의 영구 파일에는 쓰기 불가 |
| root 브로커 | root 시스템 설정, 선택한 `/dev/input` 노드, 읽기 전용 `/sys`, `/run/udev`, `/run/systemd/{seats,sessions,users}`, 상속받은 리스닝 소켓 | `/dev/uinput` 가상 상대 이동과 journal 출력. 호스트 영구 설정 및 실제 evdev에는 쓰지 않으며 시스템/사용자 D-Bus에 연결하지 않음 |
| root 대화형 설정 | 후보 터치패드 메타데이터와 읽기 전용 입력 이벤트, 선택한 설정 | 선택한 보정/설정을 원자적으로 저장. 기존 소유권, 모드, POSIX ACL, SELinux/Smack 레이블 보존 |
| 설치 프로그램/빌드 유닛 | 고정한 업스트림 소스/의존성, 시스템 툴체인, 격리된 비특권 빌드 작업 공간 | 설치 프로그램의 바이너리/유닛 세 개/해시 매니페스트, 새 설정 디렉터리, 설정 결과. 빌드 작업 공간/전용 임시 파일과 journal |

소켓의 런타임 디렉터리와 노드는 root 시스템 매니저가 생성합니다. 브로커의 전용
`/etc`, `/run`, `/var`에는 동작에 필요한 설정, 로더 캐시, 브로커 런타임 경로,
udev 데이터, 읽기 전용 logind 메타데이터만 노출합니다. 관리용 D-Bus와 systemd 전용
소켓은 숨겨 둡니다. 브로커는 버스 연결 대신 sd-login 메타데이터와 모니터를 사용합니다.
워커에서는 이 호스트 디렉터리들과 `/sys`가 숨겨집니다. 두 프로세스 모두 설치된
실행 파일과 라이브러리를 읽을 수 있습니다. 샌드박스는 별도의 OS 이미지가 아닙니다.
`DynamicUser=yes`는 영구 계정을 만드는 대신 임시 사용자 식별자를 할당합니다.
패키지 사용자도 `e-motional-lib`라는 기존 정적 계정이 없어야 합니다.
해당 계정이 있으면 systemd가 정적 사용자 식별자를 재사용하기 때문입니다.

빌드 DynamicUser는 새 `/run/e-motional-lib-build-ID/` 작업 공간
(`home`, `cargo`, `source`)과 전용 임시 파일에만 씁니다.
내려받은 압축 파일은 systemd의 읽기 전용 자격 증명 디렉터리에서 읽습니다.
root 준비 영역은 새 비공개 `/var/tmp/e-motional-lib-install.XXXXXXXX` 디렉터리를
사용하며, 설치 프로그램은 `/run/e-motional-lib-install.lock`도 확보합니다.
이 임시 프로젝트 자원은 정상 완료 또는 실패 시 제거합니다.

이 분리는 워커의 권한을 줄일 뿐, root 브로커, 설치 프로그램, 의존성, 커널의 버그를
무해하게 만들지는 않습니다. 침해된 워커는 확인된 단일 접촉/세션 상태가 허용하는 동안
제한된 포인터 이동을 요청할 수 있습니다. 출력 한 번당 각 축 최대 128 상대 이동
카운트이며, 출력 간격은 최소 8ms입니다. 높은 속도 설정도 이 안전 한도를 넘지 못합니다.
브로커의 장치 샌드박스는 입력 장치 클래스를 대상으로 하고, 애플리케이션 수준의 허용
목록이 실제로 여는 노드를 제한합니다. 원시 접촉 좌표와 장치 식별 정보는 민감한
정보이므로 로그를 공유하기 전에 검토하세요.

## 세션 및 데스크톱 호환성

logind가 보고하는 seat0의 활성 로컬 사용자 클래스 X11/Wayland 세션에서만 이동을
허용합니다. 세션 정보가 없거나, 사용할 수 없거나, 서로 맞지 않는 경우 및 원격 세션,
로그인 화면, 비그래픽 세션에서는 안전을 위해 동작을 차단합니다.
세션이 바뀌면 이동을 초기화하며 모든 손가락을 떼야 합니다.

**잠금 화면을 확실하게 감지해 차단하는 기능은 없습니다.** 컴포지터가 잠긴 그래픽
세션을 logind에서 활성 상태로 유지할 수 있으므로, 실제 포인터처럼 잠금 화면에서도
추가 포인터 이동이 계속될 수 있습니다. 잠금 힌트가 없다고 잠금이 해제되었다고
판단하지 않습니다. 모든 환경에서 잠금 화면 중 비활성을 보장해야 한다면 이 서비스를
사용하지 마세요.

uinput은 X11 인증과 Wayland 프로토콜별 입력 주입을 피하지만, 컴포지터가 가상 상대
포인터를 허용해야 합니다. Wayland/X11 동작, 가속, 가상 장치 분류, 샌드박스 기능은
커널과 데스크톱에 따라 다릅니다. 멀티시트, 원격 데스크톱, 필수 시스템 서비스/장치가
없는 컨테이너, logind를 사용하지 않는 환경은 지원하지 않습니다.

## 설정 및 운영

다시 보정하거나 다른 터치패드를 허용 목록에 추가하려면 다음을 실행합니다.

```sh
sudo systemctl stop e-motional-lib.service e-motional-lib-broker.socket e-motional-lib-broker.service
sudo e-motional-lib --setup --config-path /etc/e-motional-lib/config.toml
sudo systemctl start e-motional-lib
```

설정은 장치를 선택하고 현재 좌표의 최솟값/최댓값을 기록한 뒤 udev/하드웨어 식별 정보를 `[[devices]]`에 저장합니다.
측정하지 않은 가장자리는 추론할 수 없으므로 터치패드의 전체 표면을 인식하세요.

/etc/e-motional-lib/config.toml

```toml
edge_percent_x = 5.0
edge_percent_y = 7.0
max_speed_scale = 0.1
dwell_ms = 150
# Optional cap on each edge zone's physical width:
# edge_width_mm = 2.0
```



## 제거

소스 설치를 제거하려면 먼저 메인 서비스를 중지하고 비활성화합니다.

```sh
sudo systemctl disable --now e-motional-lib
sudo systemctl stop e-motional-lib-broker.socket e-motional-lib-broker.service
sudo sha256sum --check /var/lib/e-motional-lib-installer/manifest
```

매니페스트가 일치하고 여전히 설치 프로그램 소유의 파일인 경우에만
`/usr/bin/e-motional-lib`와 유닛 세 개
(`/usr/lib/systemd/system/e-motional-lib{,-broker}.service` 및
`/usr/lib/systemd/system/e-motional-lib-broker.socket`)를 삭제합니다.
설치 프로그램 소유의 `/var/lib/e-motional-lib-installer/manifest`와 그 빈 디렉터리를
삭제한 뒤 `sudo systemctl daemon-reload`를 실행합니다.


## 소스 구조

- `src/main.rs`: CLI 및 특권/비특권 진입점 분기.
- `src/config.rs`, `src/setup.rs`: 설정 검증과 보정.
- `src/device.rs`, `src/state.rs`: 읽기 전용 탐색, 접촉, 복구.
- `src/edge.rs`: 기하 계산, 대기 시간, 히스테리시스, 속도와 누적.
- `src/service.rs`, `src/session.rs`: 브로커/워커 및 로컬 세션 기반 허용 제어.
- `src/inject.rs`: 상대 이동만 출력하는 최소 기능 uinput.
- `packaging/`: 시스템 워커, root 브로커, root 전용 활성화 소켓.
- `install.sh`, `PKGBUILD`: 격리된 소스 설치 프로그램과 Arch 패키징.
- `LICENSE`: MIT.
