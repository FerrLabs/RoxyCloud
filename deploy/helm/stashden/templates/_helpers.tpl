{{- define "stashden.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "stashden.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else if contains (include "stashden.name" .) .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name (include "stashden.name" .) | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{- define "stashden.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "stashden.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "stashden.selectorLabels" -}}
app.kubernetes.io/name: {{ include "stashden.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "stashden.databaseSecret" -}}
{{- default (include "stashden.fullname" .) .Values.database.existingSecret }}
{{- end }}

{{- define "stashden.jwtSecret" -}}
{{- default (include "stashden.fullname" .) .Values.jwt.existingSecret }}
{{- end }}

{{- define "stashden.claimName" -}}
{{- default (include "stashden.fullname" .) .Values.persistence.existingClaim }}
{{- end }}

{{- define "stashden.s3Secret" -}}
{{- .Values.blobs.s3.existingSecret | default (include "stashden.fullname" .) -}}
{{- end -}}
