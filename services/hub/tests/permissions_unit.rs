use matehub_hub::api::auth_check::UserPerms;
use matehub_hub::models::permission::{bits, effective_permissions, has_permission, ChannelPermission};
use uuid::Uuid;

// ── Bit constants ──────────────────────────────────

#[test]
fn all_bits_covers_14_permissions() {
    assert_eq!(bits::ALL, 16383);
    assert_eq!(bits::ALL, (1 << 14) - 1);
}

#[test]
fn member_channel_preset_is_correct() {
    let expected = bits::READ | bits::WRITE | bits::CONNECT | bits::SPEAK | bits::VIDEO;
    assert_eq!(bits::MEMBER_CHANNEL, expected);
    assert_eq!(bits::MEMBER_CHANNEL, 31);
}

#[test]
fn hub_bits_dont_overlap_channel_bits() {
    let channel_mask = bits::READ | bits::WRITE | bits::CONNECT | bits::SPEAK | bits::VIDEO | bits::MANAGE_CHANNEL | bits::ADMIN_CHANNEL;
    let hub_mask = bits::CREATE_TEXT_CHANNELS | bits::CREATE_VOICE_CHANNELS | bits::EDIT_OTHER_CHANNELS | bits::MANAGE_MEMBERS | bits::INVITE_PERMANENT | bits::CREATE_TEMP_LINKS | bits::MANAGE_ROLES;
    assert_eq!(channel_mask & hub_mask, 0, "channel and hub bits must not overlap");
}

#[test]
fn each_bit_is_power_of_two() {
    let all_bits = [
        bits::READ, bits::WRITE, bits::CONNECT, bits::SPEAK, bits::VIDEO,
        bits::MANAGE_CHANNEL, bits::ADMIN_CHANNEL,
        bits::CREATE_TEXT_CHANNELS, bits::CREATE_VOICE_CHANNELS,
        bits::EDIT_OTHER_CHANNELS, bits::MANAGE_MEMBERS,
        bits::INVITE_PERMANENT, bits::CREATE_TEMP_LINKS, bits::MANAGE_ROLES,
    ];
    for (i, bit) in all_bits.iter().enumerate() {
        assert_eq!(*bit, 1 << i, "bit {i} should be {}", 1 << i);
    }
}

// ── Effective permissions ──────────────────────────

fn make_perm(allow: i32, deny: i32) -> ChannelPermission {
    ChannelPermission {
        channel_id: Uuid::nil(),
        group_id: Uuid::nil(),
        allow_bits: allow,
        deny_bits: deny,
    }
}

#[test]
fn effective_single_group_no_deny() {
    let perms = vec![make_perm(bits::READ | bits::WRITE, 0)];
    let eff = effective_permissions(&perms);
    assert!(has_permission(eff, bits::READ));
    assert!(has_permission(eff, bits::WRITE));
    assert!(!has_permission(eff, bits::CONNECT));
}

#[test]
fn effective_union_of_multiple_groups() {
    let perms = vec![
        make_perm(bits::READ, 0),
        make_perm(bits::WRITE | bits::CONNECT, 0),
    ];
    let eff = effective_permissions(&perms);
    assert!(has_permission(eff, bits::READ));
    assert!(has_permission(eff, bits::WRITE));
    assert!(has_permission(eff, bits::CONNECT));
}

#[test]
fn deny_overrides_allow() {
    let perms = vec![
        make_perm(bits::READ | bits::WRITE, 0),
        make_perm(0, bits::WRITE), // deny WRITE
    ];
    let eff = effective_permissions(&perms);
    assert!(has_permission(eff, bits::READ));
    assert!(!has_permission(eff, bits::WRITE), "deny should override allow");
}

#[test]
fn deny_from_one_group_overrides_allow_from_another() {
    let perms = vec![
        make_perm(bits::MEMBER_CHANNEL, 0),    // everyone: all channel perms
        make_perm(0, bits::WRITE | bits::VIDEO), // muted group: deny write+video
    ];
    let eff = effective_permissions(&perms);
    assert!(has_permission(eff, bits::READ));
    assert!(has_permission(eff, bits::CONNECT));
    assert!(has_permission(eff, bits::SPEAK));
    assert!(!has_permission(eff, bits::WRITE));
    assert!(!has_permission(eff, bits::VIDEO));
}

#[test]
fn empty_permissions_means_nothing_allowed() {
    let eff = effective_permissions(&[]);
    assert_eq!(eff, 0);
    assert!(!has_permission(eff, bits::READ));
}

// ── UserPerms logic ────────────────────────────────

fn make_perms(hub_bits: i32, top_position: i32, is_admin: bool, is_creator: bool) -> UserPerms {
    UserPerms { hub_bits, top_position, is_admin, is_creator }
}

#[test]
fn admin_has_all_permissions() {
    let p = make_perms(0, 1, true, false);
    assert!(p.has(bits::MANAGE_ROLES));
    assert!(p.has(bits::MANAGE_MEMBERS));
    assert!(p.has(bits::CREATE_TEXT_CHANNELS));
    assert!(p.has(bits::INVITE_PERMANENT));
}

#[test]
fn admin_can_manage_any_position() {
    let p = make_perms(0, 1, true, false);
    assert!(p.can_manage_position(0)); // even position 0
    assert!(p.can_manage_position(100));
}

#[test]
fn admin_grantable_bits_is_all() {
    let p = make_perms(0, 1, true, false);
    assert_eq!(p.grantable_bits(), bits::ALL);
}

#[test]
fn regular_user_only_has_granted_bits() {
    let p = make_perms(bits::CREATE_TEXT_CHANNELS | bits::MANAGE_MEMBERS, 2, false, false);
    assert!(p.has(bits::CREATE_TEXT_CHANNELS));
    assert!(p.has(bits::MANAGE_MEMBERS));
    assert!(!p.has(bits::MANAGE_ROLES));
    assert!(!p.has(bits::INVITE_PERMANENT));
}

#[test]
fn position_hierarchy_enforced() {
    let moderator = make_perms(bits::MANAGE_ROLES, 1, false, false);
    assert!(moderator.can_manage_position(2), "can manage lower (higher number)");
    assert!(!moderator.can_manage_position(1), "cannot manage same position");
    assert!(!moderator.can_manage_position(0), "cannot manage higher position");
}

#[test]
fn grantable_bits_limited_to_own() {
    let p = make_perms(bits::CREATE_TEXT_CHANNELS | bits::MANAGE_MEMBERS, 1, false, false);
    let grantable = p.grantable_bits();
    assert_eq!(grantable, bits::CREATE_TEXT_CHANNELS | bits::MANAGE_MEMBERS);
    // Cannot grant MANAGE_ROLES (don't have it)
    assert_eq!(grantable & bits::MANAGE_ROLES, 0);
}

#[test]
fn user_without_groups_has_no_permissions() {
    let p = make_perms(0, i32::MAX, false, false);
    assert!(!p.has(bits::READ));
    assert!(!p.has(bits::MANAGE_ROLES));
    assert!(!p.can_manage_position(0));
    assert!(!p.can_manage_position(i32::MAX)); // can't manage same
}

#[test]
fn creator_flag_independent_of_admin() {
    let creator_not_admin = make_perms(0, 5, false, true);
    assert!(!creator_not_admin.has(bits::MANAGE_ROLES), "creator without admin has no hub perms");
    assert!(creator_not_admin.is_creator);
}

// ── Bit manipulation edge cases ────────────────────

#[test]
fn has_permission_requires_all_bits_in_compound() {
    let compound = bits::READ | bits::WRITE;
    assert!(has_permission(compound, bits::READ));
    assert!(has_permission(compound, bits::WRITE));
    assert!(has_permission(compound, bits::READ | bits::WRITE));
    assert!(!has_permission(compound, bits::READ | bits::CONNECT));
}

#[test]
fn granting_only_own_bits_prevents_privilege_escalation() {
    let moderator = make_perms(
        bits::CREATE_TEXT_CHANNELS | bits::MANAGE_MEMBERS,
        1, false, false,
    );
    let requested = bits::MANAGE_ROLES; // doesn't have this
    let illegal = requested & !moderator.grantable_bits();
    assert_ne!(illegal, 0, "should detect escalation attempt");

    let requested_ok = bits::CREATE_TEXT_CHANNELS;
    let check = requested_ok & !moderator.grantable_bits();
    assert_eq!(check, 0, "granting own bits should be allowed");
}
