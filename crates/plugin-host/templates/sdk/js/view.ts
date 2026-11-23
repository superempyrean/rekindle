// rekindle/view: local overrides of how this client shows users and members,
// the current user included, and of the user's own settings (needs `view`).
// `state` and `users` queries return the overridden records. Nothing is sent
// to Discord, and the overrides go when the plugin unloads.
//
//   view.setUsers({ tag: 'others' }, [
//     { tag: 'avatar-decoration', val: undefined },
//     { tag: 'nameplate', val: undefined },
//   ]);

import * as raw from 'rekindle:plugin/view@0.10.0';
import { call, optU64, type Id } from 'rekindle';

export type Users = raw.Users;
export type UserField = raw.UserField;
export type MemberField = raw.MemberField;

export function setUsers(who: Users, fields: UserField[]): void {
  call('view.setUsers', () => raw.setUsers(who, fields));
}

/** In one guild, or every guild with `undefined`. */
export function setMembers(guild: Id | undefined, who: Users, fields: MemberField[]): void {
  call('view.setMembers', () => raw.setMembers(optU64(guild), who, fields));
}

/** One of `state.mySettingKeys()`, as its string form; `undefined` drops the override. */
export function setMySetting(key: string, value: string | undefined): void {
  call('view.setMySetting', () => raw.setMySetting(key, value));
}

export function reset(): void {
  raw.reset();
}
