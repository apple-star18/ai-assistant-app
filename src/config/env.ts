type AppEnv = 'development' | 'staging' | 'production';

interface FrontendEnv {
  appEnv: AppEnv;
  apiBaseUrl?: string;
}

const appEnv = parseAppEnv(import.meta.env.VITE_APP_ENV);
const apiBaseUrl = emptyToUndefined(import.meta.env.VITE_API_BASE_URL);

export const env: FrontendEnv = {
  appEnv,
  ...(apiBaseUrl ? { apiBaseUrl } : {}),
};

function parseAppEnv(value: string | undefined): AppEnv {
  if (value === 'staging' || value === 'production') {
    return value;
  }

  return 'development';
}

function emptyToUndefined(value: string | undefined): string | undefined {
  return value && value.trim().length > 0 ? value : undefined;
}
