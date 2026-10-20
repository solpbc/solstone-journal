import { FluentBundle, FluentVariable } from "./bundle.js";
import { ComplexPattern } from "./ast.js";
export declare class Scope {
    /** The bundle for which the given resolution is happening. */
    bundle: FluentBundle;
    /** The list of errors collected while resolving. */
    errors: Array<Error> | null;
    /** A dict of developer-provided variables. */
    args: Record<string, FluentVariable> | null;
    /**
     * The Set of patterns already encountered during this resolution.
     * Used to detect and prevent cyclic resolutions.
     * @ignore
     */
    dirty: WeakSet<ComplexPattern>;
    /** A dict of parameters passed to a TermReference. */
    params: Record<string, FluentVariable> | null;
    /**
     * The running count of placeables resolved so far.
     * Used to detect the Billion Laughs and Quadratic Blowup attacks.
     * @ignore
     */
    placeables: number;
    constructor(bundle: FluentBundle, errors: Array<Error> | null, args: Record<string, FluentVariable> | null);
    reportError(error: unknown): void;
    memoizeIntlObject(ctor: typeof Intl.NumberFormat, opts: Intl.NumberFormatOptions): Intl.NumberFormat;
    memoizeIntlObject(ctor: typeof Intl.DateTimeFormat, opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat;
    memoizeIntlObject(ctor: typeof Intl.PluralRules, opts: Intl.PluralRulesOptions): Intl.PluralRules;
}
