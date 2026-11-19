# WB Band Quantifier (wb_quant)

Western blot 밴드 강도를 ImageJ **Analyze ▸ Gels** 방식 그대로 정량하는 Rust 데스크톱 프로그램입니다.

| ImageJ 단계 | 이 프로그램 |
|---|---|
| Image ▸ Type ▸ 8-bit | 자동 (RGB는 ImageJ 기본값과 같은 (R+G+B)/3, 16-bit TIFF는 원본 bit depth 유지) |
| Edit ▸ Invert (밝은 배경 + 어두운 밴드) | `Invert: Auto` (배경 중앙값으로 자동 판단, 수동 지정 가능) |
| Gels ▸ Select First Lane / Select Next Lane | `Add lane` 도구 (첫 레인 드래그 → 클릭으로 같은 크기 레인 추가) 또는 `Draw strip` + `Auto-detect lanes` |
| Gels ▸ Plot Lanes | 오른쪽 프로파일 그래프 (레인 폭 방향 평균 = ImageJ column-average plot) |
| Straight Line 도구로 피크 바닥 닫기 | Baseline `Straight line (ImageJ)` — 녹색 경계 두 점을 잇는 직선 (자동 검출 후 그래프에서 드래그로 조정) |
| Wand 도구로 피크 클릭 → 면적 | `Peak area` = Σ(프로파일 − baseline) |
| Relative density / loading control 보정 | 하단 Results 표 (기준 레인 지정, Target ÷ LC, Normalized / ref) |

## 빌드 (Windows)

1. https://rustup.rs 에서 `rustup-init.exe` 설치 (Rust 1.85 이상, MSVC 툴체인 기본값 그대로)
2. 이 폴더에서:
   ```
   cargo build --release
   ```
3. 실행파일: `target\release\wb_quant.exe` (단일 exe, 다른 파일 불필요)

macOS / Linux도 같은 명령으로 빌드됩니다.

## 사용법 (GUI)

1. **Open target blot…** 으로 목적 단백질 이미지 열기 (TIFF/PNG/JPG/BMP, 창에 드래그&드롭도 가능)
2. (필요하면) **Rotation**으로 사진 기울기 보정: 슬라이더(0.05° 단위), 숫자 직접 입력, `-1° / -0.1° / +0.1° / +1°` 버튼. 양수 = 시계방향.
   `Grid`를 켜면 수평 가이드선이 표시되어 밴드를 수평에 맞추기 쉽습니다. 회전은 ImageJ Rotate와 같은 bilinear 보간이며, 레인 위치는 그대로 두고 회전된 이미지에서 다시 측정합니다.
3. **1. Draw strip**: 한 줄의 밴드 전체(모든 레인)를 덮는 사각형을 드래그
4. 레인 수 입력 → **2. Auto-detect lanes** (등간격 그리드를 신호에 맞춘 뒤 각 밴드 중심으로 보정 — 약한 레인이 있어도 안정적)
   - 개별 레인 조정 (`Select / move` 도구, 자동 검출 후 자동으로 선택됨):
     - 레인 안쪽을 드래그 → 이동
     - 레인의 **테두리나 모서리**를 드래그 → 그 방향으로만 크기 조절 (커서 모양이 ↔ / ↕ / ⤡ 로 바뀌는 곳)
     - 이미지에서 레인을 클릭한 뒤 **방향키** → 1 px씩 이동, **Shift+방향키** → 10 px
     - 오른쪽 패널의 `x / y / w / h` 칸에 숫자를 직접 입력하거나 좌우로 드래그해서 미세 조정
     - `Apply this size to all lanes` → 선택한 레인의 크기를 모든 레인에 적용 (각 레인 중심 유지)
     - `Delete` 키 또는 `Delete lane` 버튼 → 삭제
   - 직접 그리고 싶으면 `Add lane`: 첫 레인을 드래그로 그리고, 이후 클릭할 때마다 같은 크기 레인이 추가됩니다(ImageJ Next Lane과 동일)
5. 레인을 선택하면 오른쪽에 프로파일과 면적(주황색)이 표시됩니다.
   피크 경계(녹색)는 자동 검출되며, 그래프를 클릭/드래그하면 가까운 경계가 이동합니다. 빨간 선 = baseline.
6. **Open loading control…** 로 β-actin/GAPDH 등 이미지를 열고 같은 방법으로 레인 지정
   (같은 이미지의 다른 줄이면 같은 파일을 열고 해당 줄에 strip을 그리면 됩니다. `Copy lanes from …` 으로 레인 배치 복사 가능)
7. 하단 표에서 **Reference (control) lane** 지정 → `Normalized / ref` 가 최종 상대 발현량
8. **Export CSV…** (설정·레인 좌표·피크 경계 포함) / **Copy table** (엑셀·Prism에 붙여넣기) / **Save lane overlay PNG…** (레인 번호와 피크 경계가 그려진 기록용 이미지)

### 결과 표 열
- `Target area` : 밴드 면적 (signal × pixel, ImageJ wand 면적과 비례)
- `% of total` : ImageJ "Label Peaks" 의 퍼센트와 동일
- `Target / ref` : relative density (기준 레인 = 1)
- `LC / ref` : loading control의 relative density
- `Target ÷ LC`, `Normalized / ref` : adjusted relative density = (T/T_ref)/(LC/LC_ref)

## 사용법 (CLI, 배치 처리)

```
wb_quant.exe --cli target.tif --lanes 6 --strip 0,40,520,60 ^
             --lc actin.tif --lc-strip 0,130,520,60 --ref 1 ^
             --out results.csv --overlay target_lanes.png
```
옵션: `--rotate DEG` / `--lc-rotate DEG` (회전, 양수=시계방향), `--dir v|h` (레인 방향), `--invert auto|yes|no`, `--baseline line|min`, `--smooth N`.
`--strip` 을 생략하면 이미지 전체를 한 줄로 보고 레인을 찾습니다.

테스트용 합성 블롯 생성: `wb_quant.exe --cli --demo demo_blot.png`
(목적 단백질 상대값 1.00 / 0.80 / 0.60 / 0.40 / 0.20 / 1.20 이 나오면 정상)

## 주의사항
- 정량은 반드시 **원본(포화되지 않은) 이미지**로 하세요. JPEG 압축본이나 대비를 조정한 이미지는 값이 달라집니다. ChemiDoc 등에서는 16-bit TIFF 내보내기를 권장합니다.
- 화면 표시는 16-bit 이미지일 때 대비를 늘려 보여주지만, 계산에는 원본 값을 사용합니다.
- 면적의 절대값 단위는 ImageJ와 다르지만(ImageJ는 플롯 픽셀 단위), 레인 간 상대값은 같은 방식으로 계산됩니다.

## 구조
- `src/analysis.rs` — 프로파일, baseline, 피크 면적, 레인 자동 검출, 결과 요약, CSV, overlay (단위 테스트 포함: `cargo test`)
- `src/app.rs` — GUI (egui/eframe)
- `src/cli.rs` — 배치 모드
