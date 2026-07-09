export type AppEnvironment = 'development' | 'staging' | 'production';

export interface AppHealth {
  status: 'ok';
  version: string;
  environment: AppEnvironment;
}

export interface CommandMap {
  get_app_health: {
    args: undefined;
    response: AppHealth;
  };
}
