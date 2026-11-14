/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Which game is running, and what kind it is, so the adaptive triggers can
//! behave like a gun in shooters and like pedals in racing games.
//!
//! The running game is found from Steam's own launcher process, whose command
//! line reads `reaper SteamLaunch AppId=<id> ...`. The lists below are popular
//! Steam titles; every ID was checked against the Steam store. Add your own under
//! `[games]` in the config file.

use crate::config::Games;
use std::fs;

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

/// The kind of game `appid` is, by the user's lists first and then the built-in ones.
pub fn classify(appid: u32, extra: &Games) -> Option<Kind> {
    if extra.shooter.contains(&appid) {
        Some(Kind::Shooter)
    } else if extra.racing.contains(&appid) {
        Some(Kind::Racing)
    } else if SHOOTERS.iter().any(|g| g.0 == appid) {
        Some(Kind::Shooter)
    } else if RACING.iter().any(|g| g.0 == appid) {
        Some(Kind::Racing)
    } else {
        None
    }
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
    fn classifies_built_in_and_user_games() {
        let none = Games::default();
        assert_eq!(classify(730, &none), Some(Kind::Shooter));
        assert_eq!(classify(1551360, &none), Some(Kind::Racing));
        assert_eq!(classify(999_999_999, &none), None);
        // The user's lists win, also over the built-in ones.
        let mine = Games { shooter: vec![999_999_999], racing: vec![730] };
        assert_eq!(classify(999_999_999, &mine), Some(Kind::Shooter));
        assert_eq!(classify(730, &mine), Some(Kind::Racing));
        assert_eq!(name(1684930), Some("CULTIC"));
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
