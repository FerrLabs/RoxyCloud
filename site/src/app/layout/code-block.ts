import { ChangeDetectionStrategy, Component, input } from '@angular/core';

@Component({
  selector: 'rx-code-block',
  templateUrl: './code-block.html',
  styleUrl: './code-block.css',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class CodeBlock {
  readonly caption = input.required<string>();
  readonly code = input.required<string>();
}
