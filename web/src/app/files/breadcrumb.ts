import { ChangeDetectionStrategy, Component, computed, input } from '@angular/core';
import { RouterLink } from '@angular/router';
import { segmentsOf } from '../folder';

@Component({
  selector: 'rx-breadcrumb',
  imports: [RouterLink],
  templateUrl: './breadcrumb.html',
  styleUrl: './breadcrumb.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class Breadcrumb {
  readonly path = input.required<string>();

  protected readonly segments = computed(() => segmentsOf(this.path()));
}
