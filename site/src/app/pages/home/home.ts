import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { CONTENT } from '../../content/content';
import { withLocale } from '../../content/locale';
import { routeLocale } from '../../content/route-locale';
import { Localisation } from '../../localisation';

@Component({
  selector: 'rx-home',
  imports: [RouterLink],
  templateUrl: './home.html',
  styleUrl: './home.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Home {
  private readonly locale = routeLocale(inject(ActivatedRoute));

  protected readonly sourceUrl = ROXYCLOUD_SOURCE_URL;
  protected readonly content = CONTENT[this.locale].home;
  protected readonly installUrl = withLocale(this.locale, '/install');

  constructor() {
    inject(Localisation).describe(this.content.documentTitle, this.content.description);
  }
}
