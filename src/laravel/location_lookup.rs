//! Shared reverse lookup: given a `(Uri, Position)`, find the map entry
//! whose `Location` contains it. Each domain index (`EnvIndex`, `ConfigIndex`,
//! ...) exposes a one-line `key_at` wrapping this — used to recognize
//! "cursor is on a Laravel string-key *definition* site" for find-references.

use std::collections::HashMap;

use tower_lsp_server::ls_types::{Location, Position, Uri};

pub(super) fn key_at<'a>(
    map: &'a HashMap<String, Location>,
    uri: &Uri,
    position: Position,
) -> Option<&'a str> {
    map.iter()
        .find(|(_, loc)| {
            &loc.uri == uri && position_within(loc.range.start, loc.range.end, position)
        })
        .map(|(k, _)| k.as_str())
}

fn position_within(start: Position, end: Position, p: Position) -> bool {
    let after_start =
        p.line > start.line || (p.line == start.line && p.character >= start.character);
    let before_end = p.line < end.line || (p.line == end.line && p.character <= end.character);
    after_start && before_end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(uri: &Uri, sl: u32, sc: u32, el: u32, ec: u32) -> Location {
        Location {
            uri: uri.clone(),
            range: tower_lsp_server::ls_types::Range {
                start: Position {
                    line: sl,
                    character: sc,
                },
                end: Position {
                    line: el,
                    character: ec,
                },
            },
        }
    }

    #[test]
    fn finds_key_when_position_inside_range() {
        let uri = ("file:///a.php").parse::<Uri>().unwrap();
        let mut map = HashMap::new();
        map.insert("app.name".to_string(), loc(&uri, 2, 5, 2, 9));
        let pos = Position {
            line: 2,
            character: 7,
        };
        assert_eq!(key_at(&map, &uri, pos), Some("app.name"));
    }

    #[test]
    fn none_when_uri_differs() {
        let uri = ("file:///a.php").parse::<Uri>().unwrap();
        let other = ("file:///b.php").parse::<Uri>().unwrap();
        let mut map = HashMap::new();
        map.insert("app.name".to_string(), loc(&uri, 2, 5, 2, 9));
        let pos = Position {
            line: 2,
            character: 7,
        };
        assert_eq!(key_at(&map, &other, pos), None);
    }

    #[test]
    fn none_when_position_outside_range() {
        let uri = ("file:///a.php").parse::<Uri>().unwrap();
        let mut map = HashMap::new();
        map.insert("app.name".to_string(), loc(&uri, 2, 5, 2, 9));
        let pos = Position {
            line: 2,
            character: 20,
        };
        assert_eq!(key_at(&map, &uri, pos), None);
    }

    #[test]
    fn boundary_positions_are_inclusive() {
        let uri = ("file:///a.php").parse::<Uri>().unwrap();
        let mut map = HashMap::new();
        map.insert("app.name".to_string(), loc(&uri, 2, 5, 2, 9));
        assert_eq!(
            key_at(
                &map,
                &uri,
                Position {
                    line: 2,
                    character: 5
                }
            ),
            Some("app.name")
        );
        assert_eq!(
            key_at(
                &map,
                &uri,
                Position {
                    line: 2,
                    character: 9
                }
            ),
            Some("app.name")
        );
    }
}
