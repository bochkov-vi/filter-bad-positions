#!/usr/bin/python3


import shapefile
import json
from datetime import datetime
from geopy.distance import geodesic
from shapely.geometry import Point, Polygon
from sys import stdin



MAX_SPEED = 20 # максимальная допустимая скорость, например, 20 узлов
DIFF_SPEED = 15 # разница между расчетной скоростью и скоростью по данным терминала
SHAPEFILE = "/opt/filter/restrict.shp"  # аномальные зоны


"""Загружает полигоны из shapefile и конвертирует их в объекты Polygon."""
def load_polygons_from_shapefile(filename):

    polygons = []

    with shapefile.Reader(filename) as shp:
        for shape_rec in shp.shapeRecords():
            # Координаты каждой фигуры (shape) в shapefile
            polygon_points = shape_rec.shape.points
            # Создаем объект Polygon из координат
            polygon = Polygon(polygon_points)
            polygons.append(polygon)

    return polygons


"""Проверяет, находится ли точка внутри любого из полигонов."""
def is_point_in_polygons(point, polygons):

    for polygon in polygons:
        if polygon.contains(point):
            return True
    return False



"""Вычисляет скорость между двумя точками в узлах на основе их координат и времени."""
def calculate_speed(point1, point2):
    coords_1 = (point1['lat'], point1['lon'])
    coords_2 = (point2['lat'], point2['lon'])
    distance = geodesic(coords_1, coords_2).miles # расстояние в милях

    # Разница во времени в секундах
    time1 = datetime.fromisoformat(point1['datetime'])
    time2 = datetime.fromisoformat(point2['datetime'])
    time_diff = (time2 - time1).total_seconds() / 3600
    # Скорость в узлах
    if time_diff == 0:
        speed = -1
    else:
        speed = distance / time_diff

    return speed



"""Фильтрует траекторию на основе заданных условий."""
def filter_trajectory(data, restricted_areas, max_speed):

    filtered_trajectory = []

    for point in data:
        # Ищем нормальную начальную точку вне аномального района с адекватной скоростью в данных
        if not filtered_trajectory:
            if not is_point_in_polygons(Point(point['lon'],point['lat']), restricted_areas) and point['speed'] <= max_speed:
                filtered_trajectory.append(point)
            continue

        # Пустой оператор, но пусть будет.
        if not filtered_trajectory:
            break

        # Проверяем последующие точки
        last_point = filtered_trajectory[-1]
        calculated_speed = calculate_speed(last_point, point)

        print(f"Lat:{point['lat']} Lon:{point['lon']} Datetime:{point['datetime']} CalcSpeed:{calculated_speed}  PointSpeed:{point['speed']} DiffSpeed:{abs(calculated_speed-point['speed'])} PointInPolygone: {is_point_in_polygons(Point(point['lon'],point['lat']), restricted_areas)}")
        # Условия для добавления точки: вне аномального района, расчетная скорость и скорость от терминала меньше заданного порогового значения MAX_SPEED и  отличаются на более чем на DIFF_SPEED 
        if (calculated_speed > 0 and point['speed'] <= max_speed  and calculated_speed <= max_speed and abs(calculated_speed-point['speed']) < DIFF_SPEED and not is_point_in_polygons(Point(point['lon'],point['lat']), restricted_areas)):
            print("Point added to trajectory")
            filtered_trajectory.append(point)

    return filtered_trajectory




def main():
    polygons = load_polygons_from_shapefile(SHAPEFILE)

    filtered_data = {}
    data = json.load(stdin)['data']

    for ship in list(data.keys()):
        track = data[ship]
        track.reverse()
        filtered_track = filter_trajectory(track, polygons, MAX_SPEED)
        filtered_track.reverse()
        filtered_data.update({ship:filtered_track})

    data = {'data':filtered_data}
    json_data = json.dumps(data)

#    print(json_data)

main()