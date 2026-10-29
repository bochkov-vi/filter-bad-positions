use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;

/// Читает настройки из переменных окружения ОС.
/// Возвращает (MAX_SPEED, DIFF_SPEED, SHAPEFILE).
/// Если переменная не задана — используется значение по умолчанию.
fn load_config() -> Result<(f64, f64, String), String> {
    let max_speed = match std::env::var("MAX_SPEED") {
        Ok(v) => v.parse::<f64>().map_err(|e| format!("invalid MAX_SPEED: {}", e))?,
        Err(_) => 20.0,
    };
    let diff_speed = match std::env::var("DIFF_SPEED") {
        Ok(v) => v.parse::<f64>().map_err(|e| format!("invalid DIFF_SPEED: {}", e))?,
        Err(_) => 15.0,
    };
    let shapefile = std::env::var("SHAPEFILE").unwrap_or_else(|_| "/opt/filter/restrict.shp".to_string());

    Ok((max_speed, diff_speed, shapefile))
}

/// Точка с координатами и временем (поле JSON соответствует полям исходной Go-версии)
#[derive(Deserialize, Clone)]
struct Point {
    lat: f64,
    lon: f64,
    speed: f64,
    datetime: String,
    ship_id: i64,
    course: f64,
    source_id: i64,
    heading: f64,
    id: i64,
}

impl Point {
    fn empty() -> Point {
        Point {
            lat: 0.0,
            lon: 0.0,
            speed: 0.0,
            datetime: String::new(),
            ship_id: 0,
            course: 0.0,
            source_id: 0,
            heading: 0.0,
            id: 0,
        }
    }
}

/// Полигон как набор координат (все части shapefile "склеены" в один список,
/// как это делает исходная Go-версия через библиотеку go-shp)
struct Polygon {
    points: Vec<Point>,
}

/// Входные данные: карта "судно -> трек"
#[derive(Deserialize)]
struct InputData {
    data: BTreeMap<String, Vec<Point>>,
}

/// Выводит float в формате Go encoding/json:
/// целое значение выводится без ".0" (например, 9), иначе с дробью (9.5).
fn go_float(v: f64) -> String {
    if v == v.floor() && v.is_finite() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{}", v);
        // если получилось вида "9" (не должно), всё равно корректно
        s
    }
}

/// Экранирует строку для JSON (минимальный, но корректный экранировщик).
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Сериализует одну точку в JSON-объект с полями в том же порядке, что и Go.
fn point_to_json(p: &Point) -> String {
    format!(
        "{{\"lat\":{},\"lon\":{},\"speed\":{},\"datetime\":\"{}\",\"ship_id\":{},\"course\":{},\"source_id\":{},\"heading\":{},\"id\":{}}}",
        go_float(p.lat),
        go_float(p.lon),
        go_float(p.speed),
        json_escape(&p.datetime),
        p.ship_id,
        go_float(p.course),
        p.source_id,
        go_float(p.heading),
        p.id,
    )
}

/// Загружает полигоны из shapefile (только типы Polygon, тип 5).
/// Точки каждого полигона складываются в один общий список (Lat=Y, Lon=X),
/// чтобы в точности повторить поведение Go-библиотеки go-shp.
fn load_polygons_from_shapefile(filename: &str) -> Result<Vec<Polygon>, String> {
    let data = std::fs::read(filename).map_err(|e| e.to_string())?;
    if data.len() < 100 {
        return Err("file too small".to_string());
    }

    let mut polygons = Vec::new();
    let mut offset: usize = 100;

    while offset + 8 <= data.len() {
        // Заголовок записи: record number (BE) + content length (BE, в 16-битных словах)
        let content_len =
            u32::from_be_bytes([data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7]])
                as usize;
        let content_start = offset + 8;
        let content_end = content_start + content_len * 2;
        if content_end > data.len() {
            break;
        }
        let content = &data[content_start..content_end];

        if content.len() >= 4 {
            let shape_type = i32::from_le_bytes([content[0], content[1], content[2], content[3]]);
            // Тип 5 = Polygon
            if shape_type == 5 && content.len() >= 44 {
                let nparts = i32::from_le_bytes([content[36], content[37], content[38], content[39]]) as usize;
                let npoints = i32::from_le_bytes([content[40], content[41], content[42], content[43]]) as usize;

                let points_start = 44 + nparts * 4;
                if points_start + npoints * 16 <= content.len() {
                    let mut points = Vec::with_capacity(npoints);
                    for i in 0..npoints {
                        let start = points_start + i * 16;
                        let x = f64::from_le_bytes(content[start..start + 8].try_into().unwrap()); // долгота
                        let y = f64::from_le_bytes(content[start + 8..start + 16].try_into().unwrap()); // широта
                        let mut p = Point::empty();
                        p.lat = y;
                        p.lon = x;
                        points.push(p);
                    }
                    polygons.push(Polygon { points });
                }
            }
        }

        offset = content_end;
    }

    Ok(polygons)
}

/// Проверяет, находится ли точка внутри полигона (алгоритм ray casting),
/// в точности повторяющий логику исходной Go-версии.
fn is_point_in_polygon(point: &Point, polygon: &Polygon) -> bool {
    let mut count: i64 = 0;
    let n = polygon.points.len();

    for i in 0..n {
        let p1 = &polygon.points[i];
        let p2 = &polygon.points[(i + 1) % n];

        if (point.lat > p1.lat.min(p2.lat))
            && (point.lat <= p1.lat.max(p2.lat))
            && (point.lon <= p1.lon.max(p2.lon))
        {
            if p1.lat != p2.lat {
                let xinters = (point.lat - p1.lat) * (p2.lon - p1.lon) / (p2.lat - p1.lat) + p1.lon;
                if p1.lon == p2.lon || point.lon <= xinters {
                    count += 1;
                }
            }
        }
    }

    count % 2 != 0
}

/// Проверяет, находится ли точка внутри любого из полигонов.
fn is_point_in_polygons(point: &Point, polygons: &[Polygon]) -> bool {
    for polygon in polygons {
        if is_point_in_polygon(point, polygon) {
            return true;
        }
    }
    false
}

/// Преобразует дату-время формата "YYYY-MM-DD HH:MM:SS" в абсолютное значение в секундах
/// (время считается наивным, без часового пояса — разница двух таких величин
/// эквивалентна разнице, которую считает Go через time.Parse + Sub).
fn parse_datetime_to_seconds(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b' ' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }

    let parse_int = |b: &[u8]| -> Option<i64> {
        if b.len() == 2 && b[0].is_ascii_digit() && b[1].is_ascii_digit() {
            Some(((b[0] - b'0') as i64) * 10 + (b[1] - b'0') as i64)
        } else {
            None
        }
    };

    let year = parse_int(&bytes[0..2])? * 100 + parse_int(&bytes[2..4])?;
    let month = parse_int(&bytes[5..7])? as u32;
    let day = parse_int(&bytes[8..10])? as u32;
    let hour = parse_int(&bytes[11..13])?;
    let minute = parse_int(&bytes[14..16])?;
    let second = parse_int(&bytes[17..19])?;

    let days = days_from_civil(year, month, day);

    Some((days * 86400 + hour * 3600 + minute * 60 + second) as f64)
}

/// Количество дней от эпохи 1970-01-01 до заданной даты (алгоритм Ховарда Хиннанта).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Формула гаверсинуса: расстояние между координатами в морских милях.
/// Радиус Земли — 3440.065 морских миль (как в исходной Go-версии).
fn haversine(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 3440.065;
    let d_lat = (lat2 - lat1) * std::f64::consts::PI / 180.0;
    let d_lon = (lon2 - lon1) * std::f64::consts::PI / 180.0;

    let lat1 = lat1 * std::f64::consts::PI / 180.0;
    let lat2 = lat2 * std::f64::consts::PI / 180.0;

    let a = (d_lat / 2.0).sin() * (d_lat / 2.0).sin()
        + (lat1).cos() * (lat2).cos() * (d_lon / 2.0).sin() * (d_lon / 2.0).sin();
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    R * c
}

/// Вычисляет скорость между двумя точками в узлах по координатам и времени.
/// При ошибке разбора времени или нулевой разнице во времени возвращает -1.
fn calculate_speed(point1: &Point, point2: &Point) -> f64 {
    let distance = haversine(point1.lat, point1.lon, point2.lat, point2.lon);

    let seconds1 = parse_datetime_to_seconds(&point1.datetime);
    let seconds2 = parse_datetime_to_seconds(&point2.datetime);

    match (seconds1, seconds2) {
        (Some(t1), Some(t2)) => {
            let time_diff = (t2 - t1) / 3600.0; // часы
            if time_diff == 0.0 {
                return -1.0;
            }
            distance / time_diff
        }
        _ => -1.0,
    }
}

/// Фильтрует траекторию на основе заданных условий.
fn filter_trajectory(data: &[Point], restricted_areas: &[Polygon], max_speed: f64, diff_speed: f64) -> Vec<Point> {
    let mut filtered: Vec<Point> = Vec::new();

    for point in data {
        if filtered.is_empty() {
            if !is_point_in_polygons(point, restricted_areas) && point.speed <= max_speed {
                filtered.push(point.clone());
            }
            continue;
        }

        let last_point = filtered.last().unwrap().clone();
        let calculated_speed = calculate_speed(&last_point, point);

        if calculated_speed > 0.0
            && point.speed <= max_speed
            && calculated_speed <= max_speed
            && (calculated_speed - point.speed).abs() < diff_speed
            && !is_point_in_polygons(point, restricted_areas)
        {
            filtered.push(point.clone());
        }
    }

    filtered
}

fn reverse(points: &mut [Point]) {
    let mut i = 0;
    let mut j = points.len();
    while i < j {
        j -= 1;
        points.swap(i, j);
        i += 1;
    }
}

fn main() {
    // Загрузка настроек из переменных окружения ОС
    let (max_speed, diff_speed, shapefile) = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {}", e);
            std::process::exit(1);
        }
    };

    let polygons = match load_polygons_from_shapefile(&shapefile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error loading shapefile: {}", e);
            std::process::exit(1);
        }
    };

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("Error reading stdin: {}", e);
        std::process::exit(1);
    }

    let input_data: InputData = match serde_json::from_str(&input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error decoding JSON input: {}", e);
            std::process::exit(1);
        }
    };

    let mut filtered_data: BTreeMap<String, Vec<Point>> = BTreeMap::new();

    for (ship, mut track) in input_data.data {
        reverse(&mut track);
        let mut filtered_track = filter_trajectory(&track, &polygons, max_speed, diff_speed);
        reverse(&mut filtered_track);
        filtered_data.insert(ship, filtered_track);
    }

    // Ручной вывод JSON для точного совпадения формата чисел с Go encoding/json:
    // целые float выводятся без ".0" (например, 9), а не как 9.0.
    let mut out = String::from("{\"data\":{");
    let mut first = true;
    for (ship, track) in &filtered_data {
        if !first {
            out.push(',');
        }
        first = false;
        out.push('"');
        out.push_str(&json_escape(ship));
        out.push_str("\":[");
        for (i, p) in track.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&point_to_json(p));
        }
        out.push(']');
    }
    out.push_str("}}");

    println!("{}", out);
}
