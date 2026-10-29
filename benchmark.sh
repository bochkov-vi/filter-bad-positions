#!/usr/bin/env bash

# Бенчмарк трёх реализаций фильтрации треков AIS: Python, Go, Rust.
# Замеряется реальное время выполнения (чтение stdin -> обработка -> stdout),
# прогоном каждой версии N раз на одних и тех же входных данных.
#
# Использование:
#   ./benchmark.sh [ITERATIONS] [INPUT_FILE]
# Оба параметра необязательны:
#   ITERATIONS  — базовое число повторных прогонов для Python (по умолчанию 100)
#   INPUT_FILE  — входной JSON с треками (по умолчанию data/track-20250326.json)
#
# Быстрые языки (Go/Rust) прогоняются ITER * FAST_SCALE раз (по умолчанию 10),
# чтобы их миллисекундные времена были статистически устойчивыми.
# Среднее время считается отдельно для каждой версии — сравнение честное.
#
# Требования: утилиты date/realpath/python3, компиляторы go и cargo/rustc,
# виртуальное окружение Python (.venv).

set -euo pipefail

# --- Определение корня проекта (вне зависимости от текущей папки запуска) ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

ITER="${1:-100}"
INPUT="${2:-data/track-20250326.json}"
# Во сколько раз быстрее языки прогоняются чаще Python (можно переопределить из окружения)
FAST_SCALE="${FAST_SCALE:-10}"

INPUT="$(realpath "$INPUT")"
if [ ! -f "$INPUT" ]; then
    echo "Ошибка: входной файл не найден: $INPUT" >&2
    exit 1
fi

# --- Загрузка переменных окружения из .env (должна быть в корне проекта) ---
if [ -f "$SCRIPT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    . "$SCRIPT_DIR/.env"
    set +a
else
    echo "Предупреждение: .env не найден, используются дефолтные значения (DIFF_SPEED=15, ...)." >&2
fi

# --- Настройки путей к исполняемым файлам ---
GO_BIN="$SCRIPT_DIR/bin/track-filtering-go"
RUST_BIN="$SCRIPT_DIR/rust/target/release/track-filtering-rust"
PY_BIN="$SCRIPT_DIR/.venv/bin/python"
PY_SCRIPT="$SCRIPT_DIR/python/track_filtering.py"

# --- Проверка наличия обязательных утилит ---
missing=""
command -v date >/dev/null 2>&1 || missing="$missing date"
command -v realpath >/dev/null 2>&1 || missing="$missing realpath"
command -v python3 >/dev/null 2>&1 || missing="$missing python3"
if [ -n "$missing" ]; then
    echo "Ошибка: не найдены обязательные утилиты:$missing" >&2
    exit 1
fi

# --- Сборка Go и Rust (не учитывается в замере) ---
echo "=== Бенчмарк фильтрации треков AIS ==="
echo "Вход:      $INPUT"
echo "Итераций:  Python=$ITER; Go/Rust=$((ITER * FAST_SCALE))"
echo "----------------------------------------"
echo "[1/3] Сборка Go ..."
# GOTOOLCHAIN=local — используем установленный локальный toolchain,
# чтобы не зависеть от внешней переменной GOTOOLCHAIN (может тянуть несуществующую версию).
( cd "$SCRIPT_DIR/go" && GOTOOLCHAIN=local go build -o "$GO_BIN" . )
echo "[2/3] Сборка Rust ..."
( cd "$SCRIPT_DIR/rust" && cargo build --release --quiet )
if [ ! -x "$PY_BIN" ]; then
    echo "Внимание: $PY_BIN не найден — Python пропускается." >&2
    PY_BIN=""
fi
echo "[3/3] Замер производительности ..."
echo ""

# --- Замер одной версии: n прогонов, статистика времени в мс ---
# run_bench <метка> <входной_файл> <n_итераций> <команда...>
run_bench() {
    local label="$1"; shift
    local input="$1"; shift
    local n="$1"; shift
    local cmd=("$@")

    local -a times=()
    local total_ms=0 min_ms=0 max_ms=0

    echo "--- $label ---" >&2
    local i start end diff_ms
    for ((i=0; i<n; i++)); do
        start=$(date +%s%N)
        "${cmd[@]}" < "$input" > /dev/null
        end=$(date +%s%N)
        diff_ms=$(( (end - start) / 1000000 ))
        times+=("$diff_ms")
        total_ms=$((total_ms + diff_ms))
        if [ "$i" -eq 0 ] || [ "$diff_ms" -lt "$min_ms" ]; then min_ms=$diff_ms; fi
        if [ "$diff_ms" -gt "$max_ms" ]; then max_ms=$diff_ms; fi
    done

    local avg_ms median
    avg_ms=$((total_ms / n))
    median=$(printf '%s\n' "${times[@]}" | sort -n | awk 'NR==int((n+1)/2){print; exit}' n="$n")

    printf "      среднее:   %6d мс\n" "$avg_ms" >&2
    printf "      медиана:   %6d мс\n" "$median" >&2
    printf "      минимум:   %6d мс\n" "$min_ms" >&2
    printf "      максимум:  %6d мс\n" "$max_ms" >&2
    echo "" >&2
    echo "$avg_ms"
}

# --- Замеры: сохраняем средние для итоговой сводки ---
PY_AVG=""
if [ -n "$PY_BIN" ]; then
    PY_AVG=$(run_bench "Python" "$INPUT" "$ITER" "$PY_BIN" "$PY_SCRIPT")
else
    echo "--- Python пропущен (нет $PY_BIN) ---"
    echo ""
fi
GO_AVG=$(run_bench "Go"   "$INPUT" $((ITER * FAST_SCALE)) "$GO_BIN")
RUST_AVG=$(run_bench "Rust" "$INPUT" $((ITER * FAST_SCALE)) "$RUST_BIN")

# --- Итоговая сводка ---
echo "=== Сводка (среднее время) ==="
if [ -n "$PY_AVG" ]; then
    printf "  Python: %6d мс\n" "$PY_AVG"
    printf "  Go:     %6d мс  (x%d быстрее Python)\n" "$GO_AVG" "$((PY_AVG / GO_AVG))"
    printf "  Rust:   %6d мс  (x%d быстрее Python)\n" "$RUST_AVG" "$((PY_AVG / RUST_AVG))"
else
    printf "  Python:  <пропущен>\n"
    printf "  Go:     %6d мс\n" "$GO_AVG"
    printf "  Rust:   %6d мс\n" "$RUST_AVG"
fi
echo "----------------------------------------"

exit 0