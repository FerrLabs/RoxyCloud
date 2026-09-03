import { ChangeDetectionStrategy, Component, computed, inject } from '@angular/core';
import { Localisation } from '../localisation';

@Component({
  selector: 'rx-site-footer',
  templateUrl: './site-footer.html',
  styleUrl: './site-footer.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SiteFooter {
  private readonly localisation = inject(Localisation);

  protected readonly sourceUrl = ROXYCLOUD_SOURCE_URL;
  protected readonly licenceUrl = `${ROXYCLOUD_SOURCE_URL}/blob/main/LICENSE`;
  protected readonly footer = computed(() => this.localisation.content().chrome.footer);
}
