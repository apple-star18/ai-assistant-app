import { invoke } from '@tauri-apps/api/core';

import type { CommandMap } from './contracts';

type CommandName = keyof CommandMap;
type CommandArgs<TCommand extends CommandName> = CommandMap[TCommand]['args'];
type CommandResponse<TCommand extends CommandName> = CommandMap[TCommand]['response'];

export function getAppHealth() {
  return invokeCommand('get_app_health');
}

async function invokeCommand<TCommand extends CommandName>(
  command: TCommand,
  ...args: CommandArgs<TCommand> extends undefined ? [] : [CommandArgs<TCommand>]
): Promise<CommandResponse<TCommand>> {
  const payload = (args[0] ?? {}) as Record<string, unknown>;

  return invoke<CommandResponse<TCommand>>(command, payload);
}
