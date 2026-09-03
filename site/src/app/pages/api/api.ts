import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { ActivatedRoute } from '@angular/router';
import { CONTENT } from '../../content/content';
import { routeLocale } from '../../content/route-locale';
import { CodeBlock } from '../../layout/code-block';
import { DocTable } from '../../layout/doc-table';
import { Localisation } from '../../localisation';

@Component({
  selector: 'rx-api',
  imports: [CodeBlock, DocTable],
  templateUrl: './api.html',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Api {
  protected readonly content = CONTENT[routeLocale(inject(ActivatedRoute))].api;

  constructor() {
    inject(Localisation).describe(this.content.documentTitle, this.content.description);
  }
}
