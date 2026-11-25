#!/usr/bin/env python3
"""Makes app/lib/app_views/world.g.dart, the world Charts' Map draws, from
Natural Earth's admin-0 countries (public domain, naturalearthdata.com):
the 1:110m outlines, simplified, and every country's codes, English name
and label point from the 1:50m set. The app bundles the result: it never
fetches a map.

  curl -LO https://naciscdn.org/naturalearth/110m/cultural/ne_110m_admin_0_countries.zip
  curl -LO https://naciscdn.org/naturalearth/50m/cultural/ne_50m_admin_0_countries.zip
  unzip ne_110m_admin_0_countries.zip -d ne110 && unzip ne_50m_admin_0_countries.zip -d ne50
  python3 app/tool/natural_earth.py ne110 ne50 > app/lib/app_views/world.g.dart
"""
import base64
import glob
import os
import struct
import sys
import unicodedata


def dbf(path):
    """The records of a dBASE file, as dicts of stripped strings."""
    with open(path, 'rb') as f:
        data = f.read()
    count, header, size = struct.unpack('<IHH', data[4:12])
    fields = []
    at = 32
    while data[at] != 0x0D:
        name = data[at:at + 11].split(b'\0')[0].decode()
        length = data[at + 16]
        fields.append((name, length))
        at += 32
    out = []
    for i in range(count):
        row = data[header + i * size:header + (i + 1) * size]
        if row[:1] == b'*':
            continue
        record, at = {}, 1
        for name, length in fields:
            record[name] = row[at:at + length].decode('utf-8', 'replace').replace('\0', '').strip()
            at += length
        out.append(record)
    return out


def shapes(path):
    """Each polygon record's rings, as lists of (lon, lat)."""
    with open(path, 'rb') as f:
        data = f.read()
    at, out = 100, []
    while at < len(data):
        _, length = struct.unpack('>II', data[at:at + 8])
        body = data[at + 8:at + 8 + length * 2]
        at += 8 + length * 2
        kind = struct.unpack('<i', body[:4])[0]
        if kind != 5:
            out.append([])
            continue
        parts, points = struct.unpack('<ii', body[36:44])
        starts = struct.unpack(f'<{parts}i', body[44:44 + 4 * parts])
        xy = struct.unpack(f'<{points * 2}d', body[44 + 4 * parts:44 + 4 * parts + 16 * points])
        pts = [(xy[2 * i], xy[2 * i + 1]) for i in range(points)]
        ends = list(starts[1:]) + [points]
        out.append([pts[s:e] for s, e in zip(starts, ends)])
    return out


def simplify(points, tolerance):
    """Douglas-Peucker, keeping the ends."""
    if len(points) < 3:
        return points
    (x1, y1), (x2, y2) = points[0], points[-1]
    dx, dy = x2 - x1, y2 - y1
    norm = (dx * dx + dy * dy) ** 0.5
    worst, at = -1.0, 0
    for i in range(1, len(points) - 1):
        x, y = points[i]
        d = abs(dy * x - dx * y + x2 * y1 - y2 * x1) / norm if norm else ((x - x1) ** 2 + (y - y1) ** 2) ** 0.5
        if d > worst:
            worst, at = d, i
    if worst <= tolerance:
        return [points[0], points[-1]]
    return simplify(points[:at + 1], tolerance)[:-1] + simplify(points[at:], tolerance)


def code(record, *keys):
    for key in keys:
        value = record.get(key, '')
        if len(value) in (2, 3) and value.isalpha():
            return value.upper()
    return ''


def main(small, detailed):
    shp = glob.glob(os.path.join(small, '*.shp'))[0]
    rings = []
    for polygon in shapes(shp):
        for ring in polygon:
            ring = simplify(ring, 0.2)
            # Rings too small to see at a phone's width are left out.
            xs = [p[0] for p in ring]
            ys = [p[1] for p in ring]
            if len(ring) < 4 or (max(xs) - min(xs)) * (max(ys) - min(ys)) < 0.5:
                continue
            if min(ys) < -60:
                continue  # Antarctica: no one there to count.
            rings.append(ring)
    # Each ring: its length, then its points as tenths of a degree.
    values = []
    for ring in rings:
        values.append(len(ring))
        for lon, lat in ring:
            values += [round(lon * 10), round(lat * 10)]
    outline = base64.b64encode(struct.pack(f'<{len(values)}h', *values)).decode()

    countries = []
    # Each folded name, the codes it names: one that names two countries
    # is left out (an alias settles it).
    named = {}
    dbf50 = glob.glob(os.path.join(detailed, '*.dbf'))[0]
    for record in dbf(dbf50):
        a2 = code(record, 'ISO_A2_EH', 'ISO_A2')
        a3 = code(record, 'ISO_A3_EH', 'ISO_A3', 'ADM0_A3')
        if not a2 or not a3:
            continue
        name = _names.get(a2) or record.get('NAME') or record.get('NAME_EN')
        # Abbreviated ("Dem. Rep. Congo"): its whole name.
        if '.' in name:
            name = record.get('NAME_LONG') or name
        lon = float(record['LABEL_X'])
        lat = float(record['LABEL_Y'])
        countries.append((a2, a3, name, round(lon, 2), round(lat, 2)))
        for key in ('NAME', 'NAME_LONG', 'ADMIN', 'NAME_EN', 'FORMAL_EN', 'BRK_NAME', 'NAME_SORT'):
            if record.get(key):
                named.setdefault(fold(record[key]), set()).add(a2)
        named.setdefault(fold(name), set()).add(a2)
    for a2, a3, name, lon, lat in _more:
        countries.append((a2, a3, name, lon, lat))
        named.setdefault(fold(name), set()).add(a2)
    countries.sort()
    seen = set()
    lines = []
    for a2, a3, name, lon, lat in countries:
        if a2 in seen:
            continue
        seen.add(a2)
        lines.append(f"  ('{a2}', '{a3}', {dart_string(name)}, {lon}, {lat}),")
    names = {key: next(iter(codes)) for key, codes in named.items() if len(codes) == 1 and key}
    for alias, a2 in _aliases.items():
        assert a2 in seen, alias
        names[fold(alias)] = a2
    # Two- and three-letter names are codes, looked up as codes first.
    names = {k: v for k, v in sorted(names.items()) if not (len(k) <= 3 and k.isalpha() and k.upper() in seen)}
    # Accented Latin letters, folded as the names are, for the app to fold
    # what it reads the same way.
    folds = {}
    for rune in range(0xC0, 0x250):
        char = chr(rune)
        if not char.isalpha() or char != char.lower():
            continue
        plain = fold(char)
        if plain != char and plain.isascii() and plain:
            folds[char] = plain

    print('// Made by app/tool/natural_earth.py from Natural Earth\'s admin-0')
    print('// countries (public domain, naturalearthdata.com). Don\'t edit it by hand.')
    print('// ignore_for_file: lines_longer_than_80_chars')
    print()
    print("part of 'charts.dart';")
    print()
    print('/// The land: rings of points, each its length, then lon, lat in tenths')
    print('/// of a degree, as little-endian 16-bit integers.')
    print(f"const _outline = '{outline}';")
    print()
    print('/// Each country: alpha-2, alpha-3, its English name, and where its')
    print('/// label goes (lon, lat), which a dot marks.')
    print('const _countries = <(String, String, String, double, double)>[')
    print('\n'.join(lines))
    print('];')
    print()
    print('/// Each country by its names, folded ([_fold]): Natural Earth\'s and the')
    print('/// spellings GeoIP databases and others use.')
    print('const _countryNames = <String, String>{')
    for key, a2 in names.items():
        print(f"  {dart_string(key)}: '{a2}',")
    print('};')
    print()
    print('/// Accented Latin letters, by the plain ones [_fold] reads them as.')
    print('const _plainLetters = <String, String>{')
    for char, plain in folds.items():
        print(f"  {dart_string(char)}: {dart_string(plain)},")
    print('};')


# Shorter than Natural Earth's names, as people say them.
_names = {'US': 'United States', 'GB': 'United Kingdom'}

# Places GeoIP databases name that Natural Earth counts in another
# country: (alpha-2, alpha-3, name, lon, lat).
_more = [
    ('BQ', 'BES', 'Caribbean Netherlands', -68.26, 12.18),
    ('GF', 'GUF', 'French Guiana', -53.1, 3.9),
    ('GP', 'GLP', 'Guadeloupe', -61.55, 16.25),
    ('MQ', 'MTQ', 'Martinique', -61.0, 14.65),
    ('RE', 'REU', 'Réunion', 55.5, -21.1),
    ('YT', 'MYT', 'Mayotte', 45.15, -12.8),
]

# Other names for countries: MaxMind GeoLite2's English names where they
# differ from Natural Earth's, ISO 3166's short names, and common ones.
_aliases = {
    'Türkiye': 'TR', 'Turkiye': 'TR',
    'Czechia': 'CZ',
    'Ivory Coast': 'CI', "Cote d'Ivoire": 'CI',
    'Congo Republic': 'CG', 'Republic of Congo': 'CG', 'Congo-Brazzaville': 'CG', 'Congo': 'CG',
    'DR Congo': 'CD', 'DRC': 'CD', 'Congo-Kinshasa': 'CD', 'Democratic Republic of Congo': 'CD',
    'Congo, The Democratic Republic of the': 'CD',
    'Hong Kong': 'HK', 'Hong Kong SAR': 'HK',
    'Macao': 'MO', 'Macau': 'MO',
    'Myanmar': 'MM', 'Burma': 'MM',
    'South Korea': 'KR', 'Korea, Republic of': 'KR', 'Korea': 'KR',
    'North Korea': 'KP', "Korea, Democratic People's Republic of": 'KP',
    'Russia': 'RU',
    'Vietnam': 'VN', 'Viet Nam': 'VN',
    'Eswatini': 'SZ', 'Swaziland': 'SZ',
    'North Macedonia': 'MK', 'Macedonia': 'MK',
    'Bosnia and Herzegovina': 'BA', 'Bosnia': 'BA',
    'Palestine': 'PS', 'State of Palestine': 'PS', 'Palestinian Territory': 'PS',
    'Palestine, State of': 'PS',
    'The Netherlands': 'NL', 'Holland': 'NL',
    'Cape Verde': 'CV',
    'St Kitts and Nevis': 'KN',
    'St Lucia': 'LC',
    'St Vincent and Grenadines': 'VC', 'Saint Vincent and Grenadines': 'VC',
    'St Barthélemy': 'BL', 'Saint Barthélemy': 'BL',
    'St Martin': 'MF', 'Saint Martin': 'MF',
    'St Pierre and Miquelon': 'PM',
    'St Helena': 'SH', 'Saint Helena, Ascension and Tristan da Cunha': 'SH',
    'U.S. Virgin Islands': 'VI', 'US Virgin Islands': 'VI', 'Virgin Islands, U.S.': 'VI',
    'Virgin Islands, British': 'VG',
    'Hashemite Kingdom of Jordan': 'JO',
    'Republic of Lithuania': 'LT',
    'Republic of Moldova': 'MD', 'Moldova, Republic of': 'MD',
    'Vatican City': 'VA', 'Holy See': 'VA',
    'East Timor': 'TL',
    "Lao People's Democratic Republic": 'LA',
    'Iran, Islamic Republic of': 'IR',
    'Syrian Arab Republic': 'SY',
    'Tanzania, United Republic of': 'TZ',
    'Bolivia, Plurinational State of': 'BO',
    'Venezuela, Bolivarian Republic of': 'VE',
    'Micronesia, Federated States of': 'FM',
    'Brunei Darussalam': 'BN',
    'The Bahamas': 'BS', 'The Gambia': 'GM',
    'Taiwan, Province of China': 'TW',
    'UK': 'GB', 'Great Britain': 'GB', 'Britain': 'GB',
    'USA': 'US', 'U.S.': 'US', 'U.S.A.': 'US', 'United States of America': 'US',
    'UAE': 'AE',
    'Falkland Islands (Malvinas)': 'FK',
    'Aland Islands': 'AX',
    'Sao Tome and Principe': 'ST',
    'Curacao': 'CW',
    'Reunion': 'RE',
    'Bonaire, Sint Eustatius, and Saba': 'BQ', 'Bonaire': 'BQ',
}


def fold(text):
    """[text] as names are compared: lower case, without accents, one
    kind of apostrophe, single spaces."""
    text = unicodedata.normalize('NFKD', text.lower().replace('’', "'"))
    text = ''.join(c for c in text if not unicodedata.combining(c))
    return ' '.join(text.split())


def dart_string(text):
    return "'" + text.replace('\\', '\\\\').replace("'", "\\'").replace('$', '\\$') + "'"


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
