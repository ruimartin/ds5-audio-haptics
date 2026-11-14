/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Which game is running, and what kind it is, so the adaptive triggers can
//! behave like a gun in shooters and like pedals in racing games.
//!
//! The running game is found from Steam's own launcher process, whose command
//! line reads `reaper SteamLaunch AppId=<id> ...`. Its kind is decided, in order, by
//! the user's `[games]` lists, the curated lists below (popular titles, every ID
//! checked against the Steam store), and the game's store tags as cached on disk
//! by the Steam client. Nothing is looked up online.

use crate::config::Games;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Shooter,
    Racing,
}

pub const SHOOTERS: &[(u32, &str)] = &[
    (10, "Counter-Strike"),
    (220, "Half-Life 2"),
    (240, "Counter-Strike: Source"),
    (440, "Team Fortress 2"),
    (550, "Left 4 Dead 2"),
    (730, "Counter-Strike 2"),
    (13210, "Unreal Tournament 3"),
    (49520, "Borderlands 2"),
    (107410, "Arma 3"),
    (218620, "PAYDAY 2"),
    (221100, "DayZ"),
    (230410, "Warframe"),
    (359550, "Tom Clancy's Rainbow Six Siege"),
    (379720, "DOOM"),
    (393380, "Squad"),
    (397540, "Borderlands 3"),
    (412020, "Metro Exodus"),
    (548430, "Deep Rock Galactic"),
    (553850, "HELLDIVERS 2"),
    (578080, "PUBG: BATTLEGROUNDS"),
    (581320, "Insurgency: Sandstorm"),
    (594650, "Hunt: Showdown 1896"),
    (612880, "Wolfenstein II: The New Colossus"),
    (632360, "Risk of Rain 2"),
    (782330, "DOOM Eternal"),
    (976730, "Halo: The Master Chief Collection"),
    (1085660, "Destiny 2"),
    (1144200, "Ready or Not"),
    (1172470, "Apex Legends"),
    (1229490, "ULTRAKILL"),
    (1237970, "Titanfall 2"),
    (1238810, "Battlefield V"),
    (1238840, "Battlefield 1"),
    (1240440, "Halo Infinite"),
    (1422450, "Deadlock"),
    (1517290, "Battlefield 2042"),
    (1684930, "CULTIC"),
    (1938090, "Call of Duty"),
    (1962663, "Call of Duty: Warzone"),
    (2073850, "THE FINALS"),
    (2357570, "Overwatch"),
    (2507950, "Delta Force"),
    (2767030, "Marvel Rivals"),
    (2807960, "Battlefield 6"),
];

pub const RACING: &[(u32, &str)] = &[
    (227300, "Euro Truck Simulator 2"),
    (228380, "Wreckfest"),
    (244210, "Assetto Corsa"),
    (252950, "Rocket League"),
    (266410, "iRacing"),
    (270880, "American Truck Simulator"),
    (284160, "BeamNG.drive"),
    (310560, "DiRT Rally"),
    (365960, "rFactor 2"),
    (378860, "Project CARS 2"),
    (550320, "art of rally"),
    (646910, "The Crew 2"),
    (690790, "DiRT Rally 2.0"),
    (805550, "Assetto Corsa Competizione"),
    (1038250, "DIRT 5"),
    (1066890, "Automobilista 2"),
    (1222680, "Need for Speed Heat"),
    (1238080, "Burnout Paradise Remastered"),
    (1262540, "Need for Speed"),
    (1262560, "Need for Speed Most Wanted"),
    (1271700, "HOT WHEELS UNLEASHED"),
    (1293830, "Forza Horizon 4"),
    (1307710, "GRID Legends"),
    (1465360, "SnowRunner"),
    (1551360, "Forza Horizon 5"),
    (1692250, "F1 22"),
    (1846380, "Need for Speed Unbound"),
    (1849250, "EA SPORTS WRC"),
    (2108330, "F1 23"),
    (2225070, "Trackmania"),
    (2440510, "Forza Motorsport"),
    (2483190, "Forza Horizon 6"),
    (2488620, "F1 24"),
    (2698940, "The Crew Motorfest"),
    (3059520, "F1 25"),
];

/// Steam store tag IDs that mark a game as a shooter: FPS, Shooter, Third-Person
/// Shooter, Arena, Looter, Hero, Boomer, Extraction and On-Rails Shooter, Sniper,
/// Top-Down Shooter, Twin Stick Shooter.
const SHOOTER_TAGS: &[u32] = &[1663, 1774, 3814, 5547, 353880, 620519, 1023537, 1199779, 56690, 7423, 4637, 4758];
/// ... and as a racing game: Racing, Driving, Automobile Sim, Offroad, Motorbike,
/// Motocross, Combat Racing, ATV.
const RACING_TAGS: &[u32] = &[699, 1644, 1100687, 7622, 198913, 15868, 4102, 129761];
/// Tags are ordered by how strongly players apply them; only the leading ones count,
/// so a faint "Shooter" tag on some other kind of game is ignored.
const LEADING_TAGS: usize = 8;

/// Why a game was classified the way it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    UserList,
    BuiltInList,
    SteamTags,
}

/// Kind from store tags: whichever of the two families appears first.
pub fn kind_from_tags(tags: &[u32]) -> Option<Kind> {
    tags.iter().take(LEADING_TAGS).find_map(|t| {
        if SHOOTER_TAGS.contains(t) {
            Some(Kind::Shooter)
        } else if RACING_TAGS.contains(t) {
            Some(Kind::Racing)
        } else {
            None
        }
    })
}

/// The kind of game `appid` is: the user's lists, then the curated lists, then
/// the store tags (`tags`, when the Steam client has them cached).
pub fn classify(appid: u32, extra: &Games, tags: Option<&[u32]>) -> Option<(Kind, Source)> {
    if extra.shooter.contains(&appid) {
        Some((Kind::Shooter, Source::UserList))
    } else if extra.racing.contains(&appid) {
        Some((Kind::Racing, Source::UserList))
    } else if SHOOTERS.iter().any(|g| g.0 == appid) {
        Some((Kind::Shooter, Source::BuiltInList))
    } else if RACING.iter().any(|g| g.0 == appid) {
        Some((Kind::Racing, Source::BuiltInList))
    } else {
        tags.and_then(kind_from_tags).map(|k| (k, Source::SteamTags))
    }
}

/// What the Steam client's cache says about one app.
#[derive(Debug, Default, PartialEq)]
pub struct AppInfo {
    pub name: Option<String>,
    /// Store tag IDs, strongest first.
    pub tags: Vec<u32>,
}

/// Reads one app from the bytes of Steam's `appcache/appinfo.vdf`.
///
/// Layout (format 28 and 29): a header, then per app a 68-byte record header whose
/// second field is the record's size, followed by binary key/values. Format 29
/// stores each key as an index into a string table at the end of the file.
pub fn parse_appinfo(data: &[u8], appid: u32) -> Option<AppInfo> {
    let u32_at = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let table = match u32_at(0)? {
        0x0756_4429 => {
            // String table: a count, then that many NUL-terminated strings.
            let start = u64::from_le_bytes(data.get(8..16)?.try_into().ok()?) as usize;
            let mut strings = Vec::new();
            let mut i = start.checked_add(4)?;
            for _ in 0..u32_at(start)? {
                let end = i + data.get(i..)?.iter().position(|&b| b == 0)?;
                strings.push(&data[i..end]);
                i = end + 1;
            }
            Some(strings)
        }
        0x0756_4428 => None,
        _ => return None,
    };
    let mut pos = if table.is_some() { 16 } else { 8 };
    loop {
        let id = u32_at(pos)?;
        if id == 0 {
            return None;
        }
        let size = u32_at(pos + 4)? as usize;
        if id != appid {
            pos = pos.checked_add(8 + size)?;
            continue;
        }
        let record = data.get(pos + 68..pos + 8 + size)?;
        let mut info = AppInfo::default();
        let mut path: Vec<Vec<u8>> = Vec::new();
        let mut i = 0;
        // Key/values: a type byte, a key, a value. 0 opens a section, 8 closes one.
        while i < record.len() {
            let kind = record[i];
            i += 1;
            if kind == 8 {
                if path.pop().is_none() {
                    break;
                }
                continue;
            }
            let key: Vec<u8> = match &table {
                Some(strings) => {
                    let index = u32::from_le_bytes(record.get(i..i + 4)?.try_into().ok()?) as usize;
                    i += 4;
                    strings.get(index)?.to_vec()
                }
                None => {
                    let end = i + record.get(i..)?.iter().position(|&b| b == 0)?;
                    let key = record[i..end].to_vec();
                    i = end + 1;
                    key
                }
            };
            let at = |names: &[&str]| path.len() == names.len() && path.iter().zip(names).all(|(p, n)| p == n.as_bytes());
            match kind {
                0 => path.push(key),
                1 => {
                    let end = i + record.get(i..)?.iter().position(|&b| b == 0)?;
                    if at(&["appinfo", "common"]) && key == b"name" {
                        info.name = Some(String::from_utf8_lossy(&record[i..end]).into_owned());
                    }
                    i = end + 1;
                }
                2 => {
                    let value = u32::from_le_bytes(record.get(i..i + 4)?.try_into().ok()?);
                    if at(&["appinfo", "common", "store_tags"]) {
                        info.tags.push(value);
                    }
                    i += 4;
                }
                7 => i += 8,
                _ => return None, // a type this reader does not know: do not guess
            }
        }
        return Some(info);
    }
}

/// Where Steam keeps its app cache, for the usual install layouts.
fn appinfo_paths() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    ["/.local/share/Steam", "/.steam/steam", "/.var/app/com.valvesoftware.Steam/.local/share/Steam"]
        .iter()
        .map(|dir| PathBuf::from(format!("{}{dir}/appcache/appinfo.vdf", home.display())))
        .collect()
}

/// Looks `appid` up in the Steam client's local cache.
pub fn steam_info(appid: u32) -> Option<AppInfo> {
    appinfo_paths().iter().find_map(|path| parse_appinfo(&fs::read(path).ok()?, appid))
}

/// Name of a built-in title, for display.
pub fn name(appid: u32) -> Option<&'static str> {
    SHOOTERS.iter().chain(RACING).find(|g| g.0 == appid).map(|g| g.1)
}

/// App ID from one process's NUL-separated command line, if it is Steam's launcher.
fn appid_from_cmdline(cmdline: &[u8]) -> Option<u32> {
    let mut args = cmdline.split(|&b| b == 0);
    args.find(|a| *a == b"SteamLaunch")?;
    let id = args.next()?.strip_prefix(b"AppId=")?;
    std::str::from_utf8(id).ok()?.parse().ok().filter(|&id| id != 0)
}

/// The Steam game running right now (the most recently started one if several).
pub fn running_appid() -> Option<u32> {
    let mut best: Option<(u32, u32)> = None; // (pid, appid)
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else { continue };
        if let Some(appid) = appid_from_cmdline(&cmdline) {
            if best.map_or(true, |(p, _)| pid > p) {
                best = Some((pid, appid));
            }
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_app_id_from_steam_launcher() {
        let reaper = b"/home/deck/.local/share/Steam/steamrt64/reaper\0SteamLaunch\0AppId=1684930\0Install=1\0--\0game\0";
        assert_eq!(appid_from_cmdline(reaper), Some(1684930));
        assert_eq!(appid_from_cmdline(b"bash\0-c\0echo SteamLaunch AppId=5\0"), None);
        assert_eq!(appid_from_cmdline(b"reaper\0SteamLaunch\0AppId=abc\0"), None);
        assert_eq!(appid_from_cmdline(b"reaper\0SteamLaunch\0"), None);
        assert_eq!(appid_from_cmdline(b""), None);
    }

    #[test]
    fn classifies_by_lists_then_tags() {
        let none = Games::default();
        assert_eq!(classify(730, &none, None), Some((Kind::Shooter, Source::BuiltInList)));
        assert_eq!(classify(1551360, &none, None), Some((Kind::Racing, Source::BuiltInList)));
        assert_eq!(classify(999_999_999, &none, None), None);
        // The user's lists win over everything.
        let mine = Games { shooter: vec![999_999_999], racing: vec![730] };
        assert_eq!(classify(999_999_999, &mine, None), Some((Kind::Shooter, Source::UserList)));
        assert_eq!(classify(730, &mine, Some(&[1663])), Some((Kind::Racing, Source::UserList)));
        // Unlisted games fall back to store tags.
        assert_eq!(classify(42, &none, Some(&[19, 1663])), Some((Kind::Shooter, Source::SteamTags)));
        assert_eq!(classify(42, &none, Some(&[699, 1774])), Some((Kind::Racing, Source::SteamTags)));
        assert_eq!(classify(42, &none, Some(&[19, 21, 122])), None);
        assert_eq!(name(1684930), Some("CULTIC"));
    }

    #[test]
    fn only_leading_tags_count() {
        // "Shooter" as the 9th tag of some other kind of game does not make it one.
        assert_eq!(kind_from_tags(&[19, 21, 122, 599, 4182, 1695, 3859, 492, 1774]), None);
        assert_eq!(kind_from_tags(&[19, 21, 122, 599, 4182, 1695, 3859, 1774]), Some(Kind::Shooter));
    }

    /// A two-app cache file in format 29, built by hand.
    fn sample_appinfo() -> Vec<u8> {
        let strings: [&[u8]; 6] = [b"appinfo", b"common", b"name", b"store_tags", b"0", b"1"];
        let key = |i: u32| i.to_le_bytes();
        let record = |appid: u32, name: &[u8], tags: [u32; 2]| {
            let mut kv = Vec::new();
            kv.push(0); kv.extend(key(0)); // appinfo {
            kv.push(0); kv.extend(key(1)); //   common {
            kv.push(1); kv.extend(key(2)); kv.extend(name); kv.push(0);
            kv.push(7); kv.extend(key(2)); kv.extend([9u8; 8]); // a 64-bit value to skip
            kv.push(0); kv.extend(key(3)); //     store_tags {
            kv.push(2); kv.extend(key(4)); kv.extend(tags[0].to_le_bytes());
            kv.push(2); kv.extend(key(5)); kv.extend(tags[1].to_le_bytes());
            kv.extend([8, 8, 8]);
            let mut r = Vec::new();
            r.extend(appid.to_le_bytes());
            r.extend((60 + kv.len() as u32).to_le_bytes());
            r.extend([0u8; 60]);
            r.extend(kv);
            r
        };
        let mut body = record(111, b"First Game", [19, 21]);
        body.extend(record(222, b"Second Game", [699, 1644]));
        body.extend(0u32.to_le_bytes());
        let mut file = Vec::new();
        file.extend(0x0756_4429u32.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend((16 + body.len() as u64).to_le_bytes());
        file.extend(body);
        file.extend((strings.len() as u32).to_le_bytes());
        for s in strings {
            file.extend(s);
            file.push(0);
        }
        file
    }

    #[test]
    fn reads_name_and_tags_from_steam_cache() {
        let file = sample_appinfo();
        let second = parse_appinfo(&file, 222).unwrap();
        assert_eq!(second, AppInfo { name: Some("Second Game".into()), tags: vec![699, 1644] });
        assert_eq!(kind_from_tags(&second.tags), Some(Kind::Racing));
        assert_eq!(parse_appinfo(&file, 111).unwrap().tags, vec![19, 21]);
        assert_eq!(parse_appinfo(&file, 333), None);
    }

    #[test]
    fn damaged_cache_is_rejected_not_misread() {
        let file = sample_appinfo();
        assert_eq!(parse_appinfo(&file[..40], 222), None);
        assert_eq!(parse_appinfo(&file[..file.len() - 10], 222), None);
        assert_eq!(parse_appinfo(b"not a steam file at all", 222), None);
        assert_eq!(parse_appinfo(&[], 222), None);
        let mut wrong_version = file.clone();
        wrong_version[0] = 0x30;
        assert_eq!(parse_appinfo(&wrong_version, 222), None);
    }

    #[test]
    fn lists_have_no_duplicates() {
        let mut ids: Vec<u32> = SHOOTERS.iter().chain(RACING).map(|g| g.0).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total);
    }
}
