//! Group voice-call RPC helpers. Requests are encoded structurally (layer 227
//! constructors); large response objects are located by scanning for nested
//! constructor ids instead of carrying full generated schema decoders.
#![allow(dead_code)]

use crate::client::Client;
use crate::tl::{TlReader, TlWriter};

/// Active group call reference.
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupCallRef {
    pub id: i64,
    pub access_hash: i64,
}

// Little-endian constructor search.
pub fn find_crc(hay: &[u8], crc: u32) -> Option<usize> {
    let pat = crc.to_le_bytes();
    hay.windows(4).position(|w| w == pat)
}

/// Scans a serialized response for a full channel object and extracts
/// (id, access_hash) via the shared flags/flags2/id/access_hash prefix layout.
pub fn find_full_channel(blob: &[u8]) -> (i64, i64) {
    const CHANNEL_CRCS: [u32; 2] = [0x1c32b11c, 0xd49f34c6];
    for crc in CHANNEL_CRCS {
        let Some(p) = find_crc(blob, crc) else { continue };
        if p + 28 > blob.len() {
            continue;
        }
        let mut r = TlReader::new(&blob[p + 4..]);
        let flags = r.u32();
        let _flags2 = r.u32();
        let id = r.i64();
        let hash = if flags & (1 << 13) != 0 { r.i64() } else { 0 };
        if !r.ok() || id == 0 || hash == 0 {
            continue;
        }
        return (id, hash);
    }
    (0, 0)
}

/// channel: flags{bools} flags2 id:long access_hash:flags.13?long ...
pub fn parse_channel_object(blob: &[u8], expect_id: i64) -> Option<i64> {
    let p = match (find_crc(blob, 0x1c32b11c), find_crc(blob, 0xd49f34c6)) {
        (Some(a), Some(b)) => Some(if b < a { b } else { a }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }?;
    if p + 28 > blob.len() {
        return None;
    }
    let mut r = TlReader::new(&blob[p + 4..]);
    let f1 = r.u32();
    let _flags2 = r.u32();
    let id = r.i64();
    let mut hash = 0i64;
    if f1 & (1 << 13) != 0 {
        hash = r.i64();
    }
    if !r.ok() || id != expect_id || hash == 0 {
        return None;
    }
    Some(hash)
}

/// Reads the `megagroup` bit from a serialized channel object
/// (channel: creator:flags.0 broadcast:flags.5 megagroup:flags.8).
pub fn parse_channel_megagroup(blob: &[u8], expect_id: i64) -> Option<bool> {
    let p = match (find_crc(blob, 0x1c32b11c), find_crc(blob, 0xd49f34c6)) {
        (Some(a), Some(b)) => Some(if b < a { b } else { a }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }?;
    if p + 24 > blob.len() {
        return None;
    }
    let mut r = TlReader::new(&blob[p + 4..]);
    let flags = r.u32();
    let _flags2 = r.u32();
    let id = r.i64();
    if !r.ok() || id != expect_id {
        return None;
    }
    Some(flags & (1 << 8) != 0)
}

pub fn hex_dump(data: &[u8], n: usize) -> String {
    data.iter()
        .take(n)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

impl Client {
    /// Resolve the account-visible access_hash of a superchannel, accepting raw
 /// ids as well as marked/forms (-100xxxxxxxx); returns the resolved raw id
    pub(crate) fn resolve_channel(&mut self, chat_id: i64) -> Result<(i64, i64), String> {
        let mut candidates: Vec<i64> = Vec::new();
        let raw = if chat_id >= 0 { chat_id } else { -chat_id };
        if (100000000000..1000000000000000).contains(&raw) {
            // Marked form: -100 prefixed to the decimal digits of the channel id.
            let m = raw as u64;
            let mut pow10 = 1000000000u64;
            while pow10 <= 100000000000000u64 {
                if let Some(stripped) = m.checked_sub(100u64.checked_mul(pow10).unwrap()) {
                    if stripped < pow10 {
                        candidates.push(stripped as i64);
                        break;
                    }
                }
                pow10 *= 10;
            }
        }
        candidates.push(raw);

        let mut last_err = String::new();
        for cand in candidates {
            let mut w = TlWriter::new();
            w.u32(0x0a7f6bbb); // channels.getChannels
            w.vec_header(1);
            w.u32(0xf35aec28); // inputChannel (access_hash 0 = resolve)
            w.i64(cand);
            w.i64(0);
            let resp = self.call_rpc(w.out(), true, false);
            let resp = match resp {
                Ok(r) if !r.is_empty() => r,
                Ok(_) => {
                    last_err = "getChannels: empty response".into();
                    continue;
                }
                Err(e) => {
                    last_err = e;
                    continue;
                }
            };
            if resp.len() == 12 && find_crc(&resp, 0x64ff9fd5).is_some() {
                last_err = "channel not visible to session account (add the assistant to the group)".into();
                continue;
            }
            // Server may answer with either current constructor; both share the
            // flags/flags2/id/access_hash prefix we read.
            match parse_channel_object(&resp, cand) {
                Some(ah) => return Ok((cand, ah)),
                None => {
                    last_err = format!(
                        "channel object not found len={} {}",
                        resp.len(),
                        hex_dump(&resp, 40)
                    );
                    continue;
                }
            }
        }
        Err(last_err)
    }

    pub fn join_channel_by_username(&mut self, username: &str) -> Result<(), String> {
        let mut w = TlWriter::new();
        w.u32(0x725afbbc); // contacts.resolveUsername (layer 227: flags + referer?)
        w.u32(0); // flags: no referer
        w.str(username);
        match self.call_rpc(w.out(), true, false) {
            Err(e) => {
                if e.contains("USER_ALREADY_PARTICIPANT") {
                    return Ok(());
                }
                Err(e)
            }
            Ok(resolved) => {
                // resolvedPeer.chats carries the full channel object (id + access_hash).
                let (id, ah) = find_full_channel(&resolved);
                if ah == 0 {
                    return Err("username did not resolve to a channel".into());
                }
                self.join_channel_by_id(id, ah)
            }
        }
    }

    pub fn join_channel_by_invite(&mut self, hash: &str) -> Result<(), String> {
        let mut w = TlWriter::new();
        w.u32(0xde91436e); // messages.importChatInvite (layer 227)
        w.str(hash);
        match self.call_rpc(w.out(), true, false) {
            Err(e) => {
                if e.contains("USER_ALREADY_PARTICIPANT") {
                    return Ok(());
                }
                // INVITE_REQUEST_SENT and everything else goes back to chat_state.
                Err(e)
            }
            Ok(resp) => {
                if find_crc(&resp, 0x5a686d7c).is_some() {
                    return Ok(()); // chatInviteAlready
                }
                // chatInvite carries the target channel in its chats vector; join it.
                let (id, ah) = find_full_channel(&resp);
                if ah != 0 {
                    return self.join_channel_by_id(id, ah);
                }
                // Plain Updates (already joined / silent success).
                Ok(())
            }
        }
    }

    pub fn join_channel_by_id(&mut self, channel_id: i64, access_hash: i64) -> Result<(), String> {
        let mut w = TlWriter::new();
        w.u32(0x7f6a1e22); // channels.joinChannel (layer 227)
        w.u32(0xf35aec28); // inputChannel
        w.i64(channel_id);
        w.i64(access_hash);
        match self.call_rpc(w.out(), true, false) {
            Err(e) => {
                if e.contains("USER_ALREADY_PARTICIPANT") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
            Ok(_) => Ok(()),
        }
    }

    /// Resolve the currently active group call of a superchannel (getFullChannel).
    pub fn get_group_call(&mut self, chat_id: i64) -> Result<GroupCallRef, String> {
        let (channel_id, cah) = self.resolve_channel(chat_id)?;
        let mut w = TlWriter::new();
        w.u32(0x08736a09); // channels.getFullChannel
        w.u32(0xf35aec28); // inputChannel
        w.i64(channel_id);
        w.i64(cah);
        let resp = self.call_rpc(w.out(), true, false).map_err(|e| {
            eprintln!("[mt] getFullChannel error cid={channel_id}: {e}");
            e
        })?;
        if find_crc(&resp, 0x2633421b).is_none() && find_crc(&resp, 0xa04e8d3a).is_none() {
            // chatFull / channelFull
            return Err("getFullChannel: unexpected response shape".into());
        }
        let Some(p) = find_crc(&resp, 0xd8aa840f) else {
            // inputGroupCall
            let cf = find_crc(&resp, 0x2633421b)
                .or_else(|| find_crc(&resp, 0xa04e8d3a))
                .unwrap_or(0);
            eprintln!(
                "[mt] getFullChannel: no inputGroupCall cid={channel_id} len={} channelFull@{} {}",
                resp.len(),
                cf,
                hex_dump(&resp[cf.min(resp.len())..], 160)
            );
            return Err(format!("group call for chat {chat_id} is closed"));
        };
        if p + 20 > resp.len() {
            return Err(format!("group call for chat {chat_id} is closed"));
        }
        let mut r = TlReader::new(&resp[p + 4..]);
        let id = r.i64();
        let ah = r.i64();
        if !r.ok() || id == 0 || ah == 0 {
            // Same wording as the Go binding so the /play error classifier
            // matches and the user gets the "no active voice chat" output.
            return Err(format!("group call for chat {chat_id} is closed"));
        }
        Ok(GroupCallRef { id, access_hash: ah })
    }

    /// (id, access_hash) of our own account, cached after first call.
    /// Backfills session.user_id when the session string carried no user id.
    pub fn self_user(&mut self) -> Result<(i64, i64), String> {
        if self.self_ah != 0 && self.self_id != 0 {
            return Ok((self.self_id, self.self_ah));
        }
        let mut w = TlWriter::new();
        w.u32(0x0d91a548); // users.getUsers
        w.vec_header(1);
        w.u32(0xf7c1b13f); // inputUserSelf
        let resp = self.call_rpc(w.out(), true, false)?;
        // users.users{Vector<User>} or bare user; user#31774388 (layer 227)
        // has TWO flag words: flags:# flags2:# id:long
        // access_hash:flags.0?long ...
        // The server sends the standard constructor (not a custom crc).
        let Some(p) = find_crc(&resp, 0x31774388) else {
            return Err(format!(
                "self user object not found len={} {}",
                resp.len(),
                hex_dump(&resp, 24)
            ));
        };
        if p + 24 > resp.len() {
            return Err("self user truncated".into());
        }
        let mut r = TlReader::new(&resp[p + 4..]);
        let flags = r.u32();
        let _flags2 = r.u32();
        let id = r.i64();
        if !r.ok() {
            return Err("self user truncated".into());
        }
        if flags & 1 == 0 {
            return Err("self access hash missing".into());
        }
        // Trust the server's own id; only a real mismatch with a populated session
        // id (t.me-issued session strings embed it) is a failure.
        if self.session.user_id != 0 && id != self.session.user_id {
            return Err(format!(
                "self user mismatch id={id}/{} flags={flags:08x}",
                self.session.user_id
            ));
        }
        let ah = r.i64();
        if !r.ok() || ah == 0 {
            return Err("self access hash missing".into());
        }
        if self.session.user_id == 0 {
            self.session.user_id = id;
        }
        self.self_id = id;
        self.self_ah = ah;
        Ok((id, ah))
    }

    /// Join an active group call; returns the updateGroupCallConnection params JSON.
    pub fn join_group_call(
        &mut self,
        call: &GroupCallRef,
        params_json: &str,
        send_video: bool,
    ) -> Result<String, String> {
        let (_id, ah) = self.self_user()?;
        let mut w = TlWriter::new();
        w.u32(0x8fb53057); // phone.joinGroupCall
        // flags: bit 0x4 = video_stopped. With it set Telegram marks the
        // participant camera-off and drops every inbound video frame, so a
        // /vplay join without the bit stays a silent black screen.
        w.u32(if send_video { 0x0 } else { 0x4 });
        w.u32(0xd8aa840f); // inputGroupCall
        w.i64(call.id);
        w.i64(call.access_hash);
        w.u32(0xdde8a54c); // inputPeerUser (join as self)
        w.i64(self.session.user_id);
        w.i64(ah);
        w.u32(0x7d748d04); // dataJSON (our WebRTC offer from libntgcalls)
        w.str(params_json);
        let resp = self.call_rpc(w.out(), true, false)?;
        // updates#74ae4240 ... updateGroupCallConnection#0b783982 flags presentation;params:DataJSON
        let Some(p) = find_crc(&resp, 0x0b783982) else {
            return Err("join accepted but no connection update".into());
        };
        let mut r = TlReader::new(&resp[p + 4..]);
        let _flags = r.u32();
        let crc_obj = r.u32();
        if crc_obj != 0x7d748d04 {
            return Err("malformed connection params object".into());
        }
        let params = String::from_utf8_lossy(r.str()).into_owned();
        if !r.ok() || params.is_empty() {
            return Err("empty connection params".into());
        }
        Ok(params)
    }
}
