export type Named = {
  title: string;
  body: string;
};

export type CodeBlock = {
  caption: string;
  code: string;
};

export type TableContent = {
  columns: [string, string, string];
  rows: [string, string, string][];
};

export type Chrome = {
  skipToContent: string;
  nav: { overview: string; install: string; api: string };
  languageName: string;
  languageSwitch: string;
  footer: { tagline: string; licence: string; source: string; parent: string };
};

export type HomeContent = {
  documentTitle: string;
  description: string;
  eyebrow: string;
  heading: string;
  lead: string;
  install: string;
  source: string;
  status: {
    heading: string;
    lead: string;
    shippedHeading: string;
    shipped: string[];
    plannedHeading: string;
    planned: string[];
  };
  design: { heading: string; items: Named[] };
};

export type InstallContent = {
  documentTitle: string;
  description: string;
  heading: string;
  lead: string;
  requirements: { heading: string; items: string[] };
  compose: { heading: string; body: string; blocks: CodeBlock[]; note: string };
  source: { heading: string; body: string; blocks: CodeBlock[] };
  configuration: { heading: string; body: string } & TableContent;
  firstLogin: { heading: string; body: string; blocks: CodeBlock[] };
  webApp: { heading: string; body: string; blocks: CodeBlock[] };
};

export type ApiContent = {
  documentTitle: string;
  description: string;
  heading: string;
  lead: string;
  session: { heading: string; body: string; blocks: CodeBlock[] };
  endpoints: { heading: string; body: string } & TableContent;
  transfers: { heading: string; body: string; blocks: CodeBlock[] };
  gaps: { heading: string; body: string };
};

export type SiteContent = {
  chrome: Chrome;
  home: HomeContent;
  install: InstallContent;
  api: ApiContent;
};
