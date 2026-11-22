// Type-checked, never bundled (the entry doesn't import it): every SDK
// module, used the way its docs show, so `tsc` holds the whole TypeScript SDK
// to the contract's declarations whatever this fixture's capabilities are.

import { definePlugin, has, hex, log, rgb, span, token, type Capability, type Event, type Target } from 'rekindle';
import * as actions from 'rekindle/actions';
import * as audio from 'rekindle/audio';
import * as clipboard from 'rekindle/clipboard';
import * as commands from 'rekindle/commands';
import * as compose from 'rekindle/compose';
import { channelKind, messageFlags, perms, permissionNames } from 'rekindle/consts';
import * as decorations from 'rekindle/decorations';
import { Elements, element, uiValue } from 'rekindle/elements';
import * as events from 'rekindle/events';
import * as files from 'rekindle/files';
import * as http from 'rekindle/http';
import * as local from 'rekindle/local-actions';
import * as messages from 'rekindle/messages';
import * as notify from 'rekindle/notify';
import * as overlay from 'rekindle/overlay';
import * as users from 'rekindle/users';
import * as presence from 'rekindle/presence';
import * as reactions from 'rekindle/reactions';
import * as render from 'rekindle/render';
import * as settings from 'rekindle/settings';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import * as storage from 'rekindle/storage';
import * as theme from 'rekindle/theme';
import * as time from 'rekindle/time';
import * as timer from 'rekindle/timer';
import * as tweaks from 'rekindle/tweaks';
import * as ui from 'rekindle/ui';

export const surface = definePlugin({
  activate(granted: Capability[]) {
    if (has(granted, 'read-users')) log.info('users');
    const sel = state.selection();
    const ch = sel.channel !== undefined ? state.channel(sel.channel) : undefined;
    if (ch && channelKind.isThread(ch.kind)) log.info(ch.name);
    const can = (state.myPermissions(ch?.id ?? 0) & perms.MANAGE_MESSAGES) !== 0n;
    log.info(`${can} ${permissionNames(perms.ADMINISTRATOR).join()} ${messageFlags.EPHEMERAL}`);
    log.info(`${state.displayName(state.me().id)} ${state.unreadChannels().length} ${state.rawJson({ tag: 'none' })}`);
    log.info(`${users.typing(1).length} ${users.presence(1)?.status} ${messages.loaded(1).length} ${messages.linksIn('x')}`);
    log.info(time.format(time.snowflakeMs(1n), 'relative') + time.duration(1000));
    events.on('message-created', (m) => log.info(m.content));
    events.on('typing', ([channel, users]) => log.info(`${channel} ${users.length}`));
    const refused = events.subscribe(['message', 'typing']);
    log.info(`${refused.length}`);
    settings.onChange('keywords', () => log.info(settings.items('keywords').join()));
    settings.set('on', true);
    log.info(`${settings.bool('on')} ${settings.text('t')} ${settings.ids('u').length} ${settings.number('n')}`);
    slots.add(slots.MESSAGE_MENU, new slots.MenuItem('m', 'M').icon('copy').group('copy').when('own-message'), (t: Target) => log.info(t.tag));
    slots.messageMenuItem(new slots.MenuItem('m2', 'M2'), (m) => local.startReply(m));
    slots.userMenuItem(new slots.MenuItem('u', 'U'), (u, guild) => log.info(`${u.name} ${guild}`));
    slots.button(slots.GUILD_LIST, 'b', 'check', 'Tip', { active: true, onClick: () => local.markAllRead() });
    slots.bindClick('edit', { label: 'Edit', when: ['own-message'], modifiers: slots.mods('shift') }, (m) => local.startEdit(m));
    slots.profileSection('p', 'P', (u) => slots.setProfileSection('p', u.id, new Elements().field('a', 'b')));
    slots.setVoiceStatus('v', 'Call', 1n);
    decorations.set(decorations.NAME_BADGE, 1n, { icon: 'shield', tooltip: 'Mod' });
    decorations.onClick(decorations.NAME_BADGE, (k) => log.info(`${k}`));
    decorations.setGhost(1n, 2n, { id: 3n, name: 'x' }, 'gone');
    decorations.setDmSections([{ id: 's', name: 'S', channels: [1n] }]);
    commands.register(new commands.SlashCommand('x', 'X').text('t', 'T').choice('c', 'C', [['a', 'a']]).autocomplete(), (inv) =>
      commands.local([span(commands.text(inv, 't') ?? '', { bold: true, color: 'text-muted' })]),
    );
    new ui.Panel('p', 'P').toggle('t', 'T', true).slider('s', 'S', 1, { min: 0, max: 2 }).on('t', (v: boolean) => log.info(`${v}`)).show();
    ui.setSettingsExtra([element.button('test', 'Test')]);
    ui.registerKeybind('k', 'K', 'Ctrl+K', () => ui.toast('k'));
    ui.notice('hi', 'info', 'OK', () => ui.dismissNotice());
    theme.hideSurface('chat.gift-button');
    theme.setBackground('bg');
    theme.setRadius('radius-md', 0);
    storage.set('k', 1);
    log.info(`${storage.getMany(['k']).length} ${storage.keys('k').length} ${storage.usage().quota}`);
    clipboard.writePng(new Uint8Array());
    audio.play('x');
    tweaks.set('silent-typing', true);
    presence.setRichPresence(presence.playing('x'));
    timer.repeat('t', 100, () => undefined);
    overlay.clear('x');
    log.info(`${http.get('https://api.example.com').ok} ${files.open(['json']).name}`);
    log.info(`${uiValue.items({ tag: 'items', val: [] })} ${hex('#fff').r} ${rgb(1, 2, 3).a}`);
  },
  onEvent(ev: Event) {
    if (ev.tag === 'message-created') reactions.react(messages.reference(ev.val), '👍');
  },
  onSlot(ev) {
    const c = slots.clicked(ev);
    if (c && c.target.tag === 'message') actions.setPinned(c.target.val, true);
    const mc = slots.messageClicked(ev);
    if (mc) actions.deleteMessage(mc.message);
  },
  onCompose(draft) {
    return draft.reply ? compose.setMention(draft, false) : draft.text.includes('x') ? 'y' : compose.keep();
  },
  onUpload: () => compose.dropUpload('no'),
  render(input) {
    const t = render.highlightAll(input.message.content, 'x', { color: 'text-brand', bold: true });
    return t
      ? [t, render.background(token('background-mentioned')), render.accessory(new Elements().button('act', 'Act')), render.authorBadge({ tooltip: 'x' })]
      : [render.append([span('(x)')]), render.highlight(0, render.byteOffset(input.message.content, 1))];
  },
  onNotification: (n) => (n.body.includes('x') ? notify.decision.silent() : undefined),
  onSlashCommand: () => commands.insert('x'),
  onAutocomplete: (_c, _o, partial) => [[partial, partial]],
});
