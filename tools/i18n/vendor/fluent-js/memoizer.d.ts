export type IntlCache = Map<typeof Intl.NumberFormat | typeof Intl.DateTimeFormat | typeof Intl.PluralRules, Record<string, Intl.NumberFormat | Intl.DateTimeFormat | Intl.PluralRules>>;
export declare function getMemoizerForLocale(locales: string | string[]): IntlCache;
