import { ChangeDetectionStrategy, Component, input } from '@angular/core';

@Component({
  selector: 'rx-doc-table',
  templateUrl: './doc-table.html',
  styleUrl: './doc-table.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class DocTable {
  readonly columns = input.required<[string, string, string]>();
  readonly rows = input.required<[string, string, string][]>();
}
