# e-motional-lib

클릭 여부와 관계없이 한 손가락이 터치패드 가장자리에 도달하면 커서가 계속 움직이도록 하는 Linux/Rust 2021 데몬입니다. **evdev는 읽기 전용으로 관찰하고, 커서는 Enigo의 상대 이동 API로 움직입니다.** uinput 장치나 uinput용 udev 규칙은 사용하지 않습니다.

`-lib`는 프로젝트 이름의 일부입니다. 재사용 Rust 라이브러리 API가 아닌 **독립 실행형 데몬/CLI 프로그램**을 제공합니다.

설정 없이 기본값으로 실행할 수 있습니다. 선택적으로 `~/.config/e-motional-lib.toml`에 보정 범위와 동작 설정을 저장하며, `--setup`/`-s`로 보정할 수 있습니다. 물리 이벤트 grab·수정·차단, 버튼·키보드·휠 이벤트 전송은 하지 않습니다.

## Linux 백엔드 선택

Enigo 0.6.1 공개 릴리스를 사용합니다. 이 버전은 여러 Linux 백엔드가 연결되면 같은 이동을 각각 전송할 수 있으므로 **한 빌드에는 백엔드를 정확히 하나만** 넣습니다.

| 빌드 | 사용 환경 | 제약 |
| --- | --- | --- |
| 기본 `x11` | X11, Enigo x11rb/XTest | 로그인한 세션의 DISPLAY와 인증 정보 필요 |
| `wayland` | Enigo의 실험적 Wayland 가상 입력 프로토콜을 제공하는 compositor | virtual-pointer 및 virtual-keyboard 프로토콜과 유효한 keymap 필요; compositor마다 지원이 다름 |

**Wayland 빌드는 GNOME/KDE용 libei/RemoteDesktop portal 백엔드를 포함하지 않습니다.** 지원하지 않는 compositor에서는 연결 오류로 종료합니다. uinput이나 XWayland로 몰래 대체하지 않습니다. Wayland 세션에서 X11 빌드를 실행하면 DISPLAY를 통해 XWayland에 접속할 수 있지만, 이를 native Wayland 전체 지원으로 보장하지 않습니다.

두 기능을 동시에 켜거나 모두 끈 빌드는 거부합니다. 런타임 자동 백엔드 선택은 하지 않으며, 현재 세션에 맞는 빌드를 선택해야 합니다.

참고: [Enigo](https://github.com/enigo-rs/enigo), [공개 릴리스 Linux 구현](https://docs.rs/crate/enigo/0.6.1/source/src/linux/mod.rs).

## 빌드 및 설치

필수: stable Rust/Cargo(Enigo의 최소 Rust 버전은 1.85), C 링커, pkg-config, libudev, libxkbcommon 개발 라이브러리. Arch 예시:

```sh
sudo pacman -S --needed base-devel rust systemd libxkbcommon

# 기본 X11
cargo +stable build --release --locked
cargo +stable test --locked

# 별도 Wayland 빌드: 기존 X11 산출물을 덮어쓰지 않음
cargo +stable build --release --locked --no-default-features --features wayland --target-dir target/wayland
cargo +stable test --locked --no-default-features --features wayland --target-dir target/wayland
```

직접 설치:

```sh
# X11 빌드를 선택한 경우
sudo install -Dm755 target/release/e-motional-lib /usr/bin/e-motional-lib
# Wayland 빌드라면 대신 target/wayland/release/e-motional-lib을 설치합니다.
sudo install -Dm644 packaging/e-motional-lib.service /usr/lib/systemd/user/e-motional-lib.service
```

Arch 로컬 checkout 패키지:

```sh
makepkg -si                              # 기본 X11 패키지
E_MOTIONAL_LIB_BACKEND=wayland makepkg -fsi   # 대신 Wayland 패키지를 만들 때
```

PKGBUILD 단독 배포가 아닌 전체 checkout을 전제로 합니다. Rust 의존성은 Cargo.lock으로 고정됩니다. makepkg의 기본 `src/` 정리가 Rust 소스와 충돌하지 않도록 기본 build 디렉터리는 `target/makepkg`입니다. 두 패키지 변형은 같은 바이너리를 설치하므로 동시에 설치하지 않습니다.

### 이전 이름에서 업그레이드

기존 `libinput-edgemotion`을 사용했다면 설치 전에 구 서비스를 끄고, 수동 실행 중인 인스턴스도 종료합니다.

```sh
systemctl --user disable --now libinput-edgemotion.service
```

새 Arch 패키지 `e-motional-lib`는 구 패키지를 conflicts/replaces로 지정하므로 pacman의 교체/제거 안내를 확인합니다. 이전 버전을 **수동 설치한 경우에만** `/usr/bin/libinput-edgemotion`과 `/usr/lib/systemd/user/libinput-edgemotion.service`를 제거합니다. 구 이름의 별칭이나 서비스는 제공하지 않습니다.

이후 아래 지침대로 `e-motional-lib.service`를 등록합니다. 기존 `~/.config/e-motional-lib.toml`의 설정·보정 데이터는 그대로 사용하며 파일을 옮길 필요가 없습니다. Wayland 패키지 빌드 환경 변수도 `E_MOTIONAL_LIB_BACKEND`로 변경되었습니다.

### evdev 읽기 권한

Enigo로 커서를 움직이는 데 root/uinput 권한은 필요하지 않지만, **터치패드의 raw 이벤트를 읽는 권한은 여전히 필요**합니다. 부족하다면 배포판 정책에 맞춰 ACL을 부여하거나 다음과 같이 input 그룹을 사용합니다.

```sh
sudo usermod -aG input "$USER"
# 완전히 로그아웃하고 다시 로그인합니다.
```

**보안:** input 그룹은 키보드를 포함한 다른 입력도 읽을 수 있습니다. ACL로 읽을 수 있다면 그룹 가입은 필요 없습니다. `chmod 666`은 사용하지 마십시오.

일반 실행은 **로그인한 그래픽 세션 사용자로** 하십시오. sudo는 HOME, DISPLAY/XAUTHORITY, WAYLAND_DISPLAY/XDG_RUNTIME_DIR를 바꾸거나 세션 소켓 접근을 막을 수 있습니다. evdev 권한 문제를 해결하기 위해 무조건 root로 데몬을 실행하는 방식은 권장하지 않습니다.

### 이전 uinput 버전에서 전환

기존 데몬/서비스를 먼저 종료하고 새 바이너리와 user unit을 설치합니다. 이전 버전에 따라 직접 설치했던 전용 규칙은 더 이상 필요하지 않습니다.

```sh
# 아래 두 파일은 이 프로젝트의 이전 규칙을 직접 설치했던 경우에만 제거합니다.
sudo rm -f /etc/udev/rules.d/99-edgemotion-uinput.rules /usr/lib/udev/rules.d/99-edgemotion-uinput.rules
sudo udevadm control --reload-rules
```

다른 앱이 uinput을 사용할 수 있으므로 uinput 모듈 자체를 제거/언로드할 필요는 없습니다. 기존 TOML 설정·보정 데이터는 그대로 사용합니다. Enigo 출력에서는 udev의 `ID_INPUT_MOUSE` 분류가 필요하지 않습니다.

## 실행과 사용자 서비스

```sh
e-motional-lib --list-devices
e-motional-lib                      # foreground, 종료: Ctrl-C
```

서비스와 foreground를 동시에 실행하면 이동이 중복됩니다. 한 인스턴스만 실행하십시오.

user unit은 그래픽 세션에 연결됩니다. 이전 default.target 기반 unit을 사용했다면 enable 링크를 갱신합니다. 현재 X11 터미널에서의 예:

```sh
systemctl --user disable --now e-motional-lib.service
systemctl --user daemon-reload
systemctl --user import-environment DISPLAY XAUTHORITY XDG_SESSION_TYPE
systemctl --user enable --now e-motional-lib.service
journalctl --user -u e-motional-lib.service -f
```

Wayland에서는 import-environment에 `WAYLAND_DISPLAY XDG_RUNTIME_DIR XDG_SESSION_TYPE`를 전달합니다. 일반적인 데스크톱은 세션 환경을 user manager에 이미 전달합니다. graphical-session.target을 관리하지 않는 데스크톱에서는 foreground 실행이나 명시적인 user 서비스 시작이 필요할 수 있습니다.

서비스는 HOME를 읽기 전용으로 허용해 TOML을 읽습니다. 임시 디렉터리는 Enigo의 Wayland keymap 파일 생성을 위해 쓰기 가능하게 유지합니다. 보정은 서비스가 아니라 foreground `--setup`으로 수행합니다.

## 선택 인자

| 인자 | 기본값 | 의미 |
| --- | --- | --- |
| `--edge-width-mm` | 파일 값; 없으면 상한 없음 | 축별 비율로 계산한 폭의 mm 상한을 덮어씀 |
| `--max-speed-scale` | 파일 값; 없으면 0.3 | 최대 속도 배율을 덮어씀 |
| `--dwell-ms` | 파일 값; 없으면 150 | edge 체류 시간을 덮어씀; 0도 허용 |
| `--list-devices` | off | 장치·보정 ID·적용 범위를 출력하고 종료; 그래픽 연결 불필요 |
| `--setup`, `-s` | off | 좌표 범위·가로/세로 edge 비율을 보정하고 저장 후 종료 |
| `--verbose` | off | 설정·장치·출력 연결·활성 전이 debug 로그 |
| `--dbg`, `-d` | off | timestamp 포함 원시 이벤트·프레임·판정·타이머·Enigo 이동 trace |

mm 상한과 속도 배율은 양수 유한수이며 최대 1000000입니다. `RUST_LOG`로 로그 필터를 지정할 수 있습니다. `-d`는 이 바이너리의 trace를 명시적으로 활성화하므로 해당 필터보다 우선합니다.

## 설정 파일과 캘리브레이션

일반 실행은 **`$HOME/.config/e-motional-lib.toml`**을 시작할 때 한 번 읽습니다. 파일이 없으면 생성하지 않고 기본값(가로 5%, 세로 5%, 속도 배율 1, dwell 150ms, EVIOCGABS 좌표 범위)을 사용합니다. 잘못된 파일은 기본값으로 숨기지 않고 경로와 원인을 출력한 뒤 종료합니다. 변경 후 데몬을 재시작하십시오.

우선순위는 **CLI > 파일 > 기본값**입니다. 일반 실행의 CLI 덮어쓰기는 파일을 수정하지 않습니다. `--setup`과 함께 준 폭 상한·속도·dwell 옵션은 보정 결과와 함께 저장합니다.

```sh
systemctl --user stop e-motional-lib.service
e-motional-lib --setup              # 같은 옵션: -s
e-motional-lib
```

setup/list-devices는 Enigo 연결을 초기화하지 않으므로 그래픽 세션이 없어도 evdev 읽기 권한만 있으면 실행할 수 있습니다. setup과 일반 실행의 HOME를 동일하게 유지하십시오. sudo 사용 시 `/root/.config/e-motional-lib.toml`과 사용자 설정이 달라질 수 있습니다. 화면에 표시되는 실제 저장/로딩 경로를 확인하십시오.

1. 보정할 패드를 번호로 선택합니다. Enter는 첫 장치입니다.
2. 손가락을 모두 뗀 후 한 손가락으로 **네 가장자리와 모서리 전체**를 충분히 훑습니다. 클릭은 필요 없습니다. live `Observed raw ranges`에서 최소·최대값을 확인합니다.
3. Enter로 수집을 마칩니다. 두 축 모두 이동 범위가 필요합니다. `q` 후 Enter, Ctrl-C, stdin EOF는 저장 없이 취소합니다. overflow에서는 샘플을 폐기하고 전체 lift 후 다시 시작합니다.
4. 좌우와 위아래 edge 비율을 각각 입력합니다. Enter는 현재 값 유지이며, `0 < 비율 < 50` 범위입니다.
5. 선택한 장치의 보정을 교체하고 다른 장치는 유지한 채 원자적으로 저장합니다. TOML 주석/서식은 재생성됩니다.

`origin: Some([2065.0, 946.0])`은 **접촉 시작 좌표**이지 최대값이 아닙니다. 보정은 SYN_REPORT별 **현재 접촉 위치**의 최솟값/최댓값을 수집합니다. 최소값을 0으로 가정하지 않습니다. 닿지 않은 영역의 끝은 추정하지 않으므로 부분적으로만 훑으면 잘못 보정될 수 있습니다.

전역 설정 예시:

```toml
edge_percent_x = 8.0
edge_percent_y = 3.0
max_speed_scale = 1.0
dwell_ms = 150
# 선택 사항: edge_width_mm = 2.0
```

`edge_percent_x`는 너비에 대한 좌우 각각의 비율, `edge_percent_y`는 높이에 대한 위아래 각각의 비율입니다. setup은 `[[devices]]`에 `id`, `min_x`, `max_x`, `min_y`, `max_y`를 저장합니다. event 번호 대신 udev 포트/하드웨어 경로와 장치 이름으로 대응합니다. 보정이 없는 패드는 장치 보고 범위를 사용합니다. 비율은 모든 패드에 적용되는 전역 값이며 여러 패드는 setup을 반복해 보정합니다.

## 동작 조건과 속도

- udev `ID_INPUT_TOUCHPAD=1`의 지원 event 노드를 관찰하고 hotplug를 처리합니다. MT X/Y를 우선하고 ABS_X/Y로 fallback합니다. 기존 BTN_LEFT 또는 BTN_TOOL_FINGER capability 검사와 접촉 추적은 유지하지만, 버튼을 누르고 있는지는 활성화 조건이 아닙니다.
- MT type B slot·tracking ID, type A, single-touch를 지원합니다. PALM은 제외합니다. 위치가 불명확한 접촉도 수에는 포함시켜 잘못된 한 손가락 판정을 막습니다.
- **비-palm 손가락 정확히 하나 + 접촉 시작점이 모든 edge zone 밖 + 현재 edge 안 + 기본 150ms 체류**가 필요합니다. 일반 커서 이동과 클릭 드래그 모두 동일하게 판정하며, edge에서 손가락을 멈춰도 조건을 유지하면 계속 움직입니다.
- 기본 zone은 좌우 각각 너비의 5%, 위아래 각각 높이의 5%입니다. 보정 범위와 설정된 축별 비율을 사용합니다. mm 상한이 있으면 추가 제한합니다.
- 축별 dwell과 활성 후 약 1mm inward hysteresis를 사용합니다. dwell 중 zone을 벗어나면 취소하며, 활성 후 hysteresis 구간에서는 최소 속도를 유지합니다. 코너에서는 두 축이 각각 준비되면 대각선으로 움직입니다.
- lift·두 번째 손가락·zone 이탈(hysteresis 적용)·장치 제거 시 정지하고 소수 누적을 지웁니다. **클릭 버튼을 누르거나 해제하는 것만으로는 dwell이나 이동을 중단하지 않습니다.** 입력 프레임을 타이머 출력보다 먼저 처리합니다. 실제 정지 지연에는 장치 프레임·OS 스케줄링·그래픽 서버 왕복 시간이 영향을 줍니다.
- 시작 시 이미 닿은 접촉, SYN_DROPPED 후의 접촉은 전체 lift 후 새 접촉부터 활성화합니다. MT 좌표의 커널 변경분 전송을 고려해 축 캐시를 유지합니다.
- 두 축 resolution이 1 unit/mm 초과이고 각 변이 10–500mm이면 실제 mm를 사용합니다. 아니면 raw 축 비율을 유지한 긴 변 100mm를 사용합니다. fallback의 mm/hysteresis는 실제 측정값이 아닙니다.
- depth는 축별 boundary=0, 물리 edge=1입니다. `speed = v_min + (v_max-v_min) × depth^1.5`, `v_min = 0.1 × v_max`입니다.
- 속도 수치는 기존 기준을 유지합니다: `v_max = 긴 변(mm) × (1000/25.4) / 1.5 × 배율`. 이제 이 값은 Enigo 상대 좌표 단위/초이며 X11에서는 픽셀/초입니다. compositor의 배율/가속을 역산하지 않으며 실제 화면을 1.5초에 횡단한다는 보장은 없습니다. 필요하면 `max_speed_scale`로 조절합니다.
- 장치·축별 signed 소수 누적으로 이동 손실을 줄이고 여러 패드의 이동을 합산합니다. Enigo에는 0이 아닌 정수 상대 이동만 전달합니다. 버튼·키보드·휠 API는 호출하지 않으며 자동 키 해제도 비활성화합니다.
- 단일 스레드 epoll이 evdev, udev monitor, CLOCK_MONOTONIC timerfd, signalfd를 감시합니다. 활성 중에만 125Hz timerfd를 사용하고 실제 dt로 적분합니다. dwell 만료에는 일회성 epoll timeout을 사용하며 완전 idle에서는 무기한 대기합니다.
- 별도의 tap-to-drag 설정 감지나 탭/클릭 합성은 하지 않습니다. 활성화가 버튼 상태와 무관하므로 그런 drag 상태를 추측할 필요가 없습니다. GUI, gesture, scroll, D-Bus 제어 API도 없습니다.

### 이동 속도 조절

`~/.config/e-motional-lib.toml`의 **기존 최상위 `max_speed_scale`** 값을 바꿉니다. `[[devices]]` 항목 안이 아니라 그 앞의 전역 설정입니다.

```toml
max_speed_scale = 0.5
```

| 배율 | 배율 1.0 기준 상대 속도 |
| --- | --- |
| `0.3` | 30% — 설정이 없을 때 기본값 |
| `0.5` | 50% |
| `1.0` | 100% |
| `2.0` | 200% |

최소·최대 속도가 모두 같은 배율로 바뀌며 edge 안쪽 깊이에 따른 가속 곡선은 유지합니다. 양수만 허용합니다. 파일 수정 후에는 데몬/서비스를 재시작해야 합니다.

파일을 바꾸지 않고 시험하려면 기존 인스턴스를 먼저 종료한 뒤 실행합니다.

```sh
./target/debug/e-motional-lib --max-speed-scale 0.5 -d
```

이 CLI 값이 파일보다 우선하며 파일에 저장되지는 않습니다. `dwell_ms`/`--dwell-ms`는 이동 속도가 아니라 활성화 대기 시간입니다.

## 상세 로그

새 바이너리를 로그인한 그래픽 세션 터미널에서 실행합니다.

```sh
./target/debug/e-motional-lib -d 2>&1 | tee e-motional-lib.log
```

stderr에 출력하며 별도 파일은 자동 생성하지 않습니다. 위 명령은 현재 디렉터리에 로그를 저장합니다.

- 시작/설정: 버전, UID/EUID, config 경로, 적용된 비율·속도·dwell
- 장치: 읽기 전용 open, 제외 이유, 보고/보정 범위, 보정 ID, resolution, 초기 격리 상태
- 판정: 원시 evdev 이벤트, SYN_REPORT별 BTN_LEFT·접촉·origin, 차단 사유, 방향·depth·dwell
- 출력: `cursor output: Enigo X11` 또는 `Enigo Wayland`, `velocity_px_s`, 실제 dt·`delta_px`, 성공한 `Enigo ... moved pointer dx=... dy=...`

**주의:** 좌표·시간·장치 식별 정보가 포함됩니다. 공유 전 확인하십시오. 활성 중 로그량과 오버헤드가 크므로 평상시에는 끄십시오. `--verbose`는 원시 이벤트·프레임·매 tick trace를 제외합니다.

## 검증과 수용 기준

X11/Wayland 각각의 빌드에서 하드웨어 없는 테스트 39개가 통과했습니다. 버튼 없는 활성화·버튼 변화에 독립적인 dwell/이동, zone·depth·속도·accumulator·dwell/hysteresis, 접촉 추적, 설정 검증/저장, 보정 범위 및 장치별 적용을 검증합니다. 두 백엔드의 Clippy도 통과했습니다.

실행 smoke에서는 **실제 Xvfb/X11 서버의 커서 좌표**를 읽어 상대 이동을 확인했습니다. BTN_LEFT 이벤트 없는 replay 접촉 프레임 → 5% edge → 실제 dwell → 소수 누적 → Enigo 경로에서 커서가 이동했습니다. 속도 배율별 좌표 변화, 버튼을 눌렀다 떼어도 이동 유지, 두 번째 손가락에서 정지를 확인했습니다. 앞선 native 출력 검증에서는 `/dev/uinput`을 uinput ioctl이 불가능한 노드로 가린 경우의 이동과, 장치 없는 실제 데몬의 1초간 idle CPU tick 증가 0 및 SIGTERM 종료 코드 0도 확인했습니다.

Wayland는 컴파일·테스트와 사용 불가능한 display에 대한 오류 종료를 확인했습니다. **실제 Wayland compositor의 커서 이동은 미검증**입니다. 실제 물리 터치패드를 읽는 권한도 없어 물리 패드 전체 경로를 이 환경에서 실행하지 못했습니다.

다음은 실제 장치에서 확인할 항목입니다. 도구 예시: `sudo pacman -S libinput evtest evemu strace sysstat`.

| 기준 | 확인 방법 |
| --- | --- |
| 물리 입력 불변·grab 없음 | 데몬과 `sudo evtest /dev/input/eventX` 또는 `sudo libinput debug-events`를 병행합니다. 물리 패드에 출력 이벤트가 추가되지 않아야 합니다. `--grab`은 쓰지 않습니다. 필요하면 strace에서 O_RDONLY 및 grab ioctl 부재를 확인합니다. |
| edge drift·깊이에 따른 가속 | 중앙에서 한 손가락을 edge로 이동하고 150ms 정지합니다. 클릭 없이, 클릭한 상태에서 각각 시험합니다. 네 변·코너 및 끝으로 갈수록 빨라지는 이동을 관찰합니다. |
| 버튼 변화 무관·lift/두 손가락 정지 | 버튼을 눌렀다 떼어도 손가락이 edge에 있으면 계속 움직여야 합니다. lift·두 번째 손가락·zone 이탈 후에는 추가 이동이 멈춰야 합니다. |
| edge thumb 방지 | edge에서 접촉을 시작한 손가락은 버튼 상태와 무관하게 중앙을 거쳐 돌아와도 활성화되지 않아야 합니다. |
| SIGKILL 안전성 | 서비스를 끄고 foreground 데몬의 drift 중 다른 터미널에서 `kill -KILL <PID>`를 실행합니다. 물리 클릭 해제·이동이 정상이어야 합니다. |
| idle CPU | 손가락을 떼고 `pidstat -p <PID> 1`로 확인합니다. |

### evemu 녹화/재생

재생은 실제 데스크톱 입력을 발생시키므로 안전한 세션에서 수행합니다. 물리 장치에 `evemu-play`를 실행하지 않습니다.

```sh
systemctl --user stop e-motional-lib.service
sudo evemu-describe /dev/input/eventX touchpad.desc
sudo evemu-record /dev/input/eventX drag.events
```

전체 lift 상태에서 녹화를 시작한 뒤 **버튼을 누르지 않고** 중앙 접촉 → edge → 최소 0.5초 정지 → 전체 lift를 기록하고 Ctrl-C로 마칩니다. 버튼을 눌렀다 떼는 동안 이동 유지, 두 손가락·edge-start·코너도 별도로 기록합니다.

1. 터미널 A에서 `sudo evemu-device touchpad.desc`를 유지하고 새 가상 터치패드 `eventY`를 기록합니다. 물리 eventX와 달라야 합니다.
2. `sudo udevadm settle` 후 eventY의 `ID_INPUT_TOUCHPAD=1`을 확인합니다. 원본이 분류 quirk를 요구했다면 가상 장치에도 그 분류가 필요합니다.
3. evemu가 만든 eventY를 **그래픽 세션 사용자**가 읽을 수 있어야 합니다. input 그룹/한정된 ACL로 읽기 권한을 준비합니다. 이 데몬 자체에는 uinput 권한이 필요 없지만, evemu-device는 테스트 장치 생성에 uinput을 사용합니다.
4. 터미널 B에서 세션 사용자로 `e-motional-lib -d`를 실행하고 eventY 관찰 로그를 확인합니다.
5. 터미널 C에서 `sudo evemu-play /dev/input/eventY < drag.events`를 실행합니다. dwell 이후 커서 이동과 `Enigo ... moved pointer` 로그, lift 후 정지를 확인합니다. 버튼 해제만으로는 정지하지 않아야 합니다.
6. evemu-device 종료 시 hot-remove 후 정지하는지도 확인하고 모든 검증 프로세스를 종료합니다.

Enigo 출력은 물리 evdev 노드에 쓰이지 않으므로 evtest에서 출력 이동을 찾는 방식으로 검증하지 않습니다. X11은 커서 위치/화면, Wayland는 compositor 화면을 관찰합니다.

참조: [evemu-record](https://manpages.debian.org/testing/evemu-tools/evemu-record.1.en.html), [evemu-play](https://manpages.debian.org/testing/evemu-tools/evemu-play.1.en.html).

## 문제 해결

- `Enigo X11 connection failed`: X11 빌드인지, DISPLAY/XAUTHORITY와 현재 사용자 인증이 맞는지 확인합니다. SSH/headless/root 터미널 대신 로그인한 데스크톱 사용자 터미널에서 실행하십시오.
- `Enigo Wayland connection failed`: Wayland 빌드와 WAYLAND_DISPLAY/XDG_RUNTIME_DIR를 확인합니다. compositor가 Enigo의 가상 입력 프로토콜/keymap을 제공하지 않으면 이 빌드로 제어할 수 없습니다. libei/portal 자동 대체는 없습니다.
- evdev `permission denied`: 해당 event 노드의 ACL/input 그룹·재로그인을 확인합니다. 예: `getfacl /dev/input/eventX`. Enigo 연결 권한과 별개의 문제입니다.
- 장치 없음: `udevadm info -q property -n /dev/input/eventX`의 `ID_INPUT_TOUCHPAD=1` 및 ABS/KEY capability를 확인합니다. 데몬은 hotplug를 기다립니다.
- deadline은 지나는데 이동 없음: 손가락 수, `origin` 차단, `quarantined`를 확인한 다음 `motion activated` → `timerfd armed` → `delta_px` → `Enigo ... moved pointer` 순서의 로그를 확인합니다. BTN_LEFT는 더 이상 활성화 조건이 아닙니다. 그래픽 세션 연결 실패/잘못 선택한 백엔드는 보정값으로 해결되지 않습니다.
- 클릭/탭 드래그 설정: 해당 설정을 감지하지 않으며 버튼 누름·해제와 무관하게 접촉 위치로 판정합니다. 클릭이나 탭 자체를 합성하지 않습니다.
- 시작/overflow 직후 정지: 전체 lift 후 중앙에서 새 접촉을 시작합니다.
- 너무 빠름/느림: `max_speed_scale` 또는 `--max-speed-scale`로 조절합니다.
- 설정 파일 오류: 표시된 경로/필드와 `0 < edge_percent_x/y < 50`, 장치별 유한한 min < max를 확인합니다. 잘못된 파일을 조용히 무시하지 않습니다.
- 서비스에서만 실패: `systemctl --user show-environment`와 journal에서 그래픽 세션 환경을 확인하고 unit 재설치/daemon-reload 후 `reset-failed` 및 `restart`를 실행합니다.

## 파일 구성

- `Cargo.toml`, `Cargo.lock`: Enigo 백엔드 feature와 고정 의존성
- `src/main.rs`: CLI, epoll/timerfd/signalfd 루프
- `src/config.rs`: TOML 기본값·검증·읽기·원자적 저장
- `src/setup.rs`: read-only 좌표 보정과 가로·세로 비율 입력
- `src/device.rs`: udev discovery/hotplug, 읽기 전용 evdev, 보정 적용·복구
- `src/state.rs`: 프레임 단위 접촉·버튼 추적
- `src/edge.rs`: 상수, geometry·활성화·속도·소수 누적
- `src/inject.rs`: 단일 Enigo 백엔드의 상대 이동만 호출
- `packaging/e-motional-lib.service`: 그래픽 세션용 user unit
- `PKGBUILD`: X11 기본/Wayland 선택 Arch 패키지
- `LICENSE`: MIT
