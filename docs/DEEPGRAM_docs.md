# Live Audio

GET /v1/listen

Transcribe audio and video using Deepgram's speech-to-text WebSocket

Reference: https://developers.deepgram.com/reference/speech-to-text/listen-streaming

## AsyncAPI Specification

```yaml
asyncapi: 2.6.0
info:
  title: listen.v1
  version: subpackage_listen/v1.listen.v1
  description: Transcribe audio and video using Deepgram's speech-to-text WebSocket
channels:
  /v1/listen:
    description: Transcribe audio and video using Deepgram's speech-to-text WebSocket
    bindings:
      ws:
        query:
          type: object
          properties:
            callback:
              $ref: '#/components/schemas/ListenV1Callback'
            callback_method:
              $ref: '#/components/schemas/ListenV1CallbackMethod'
            channels:
              $ref: '#/components/schemas/ListenV1Channels'
            detect_entities:
              $ref: '#/components/schemas/ListenV1DetectEntities'
            diarize:
              $ref: '#/components/schemas/ListenV1Diarize'
            dictation:
              $ref: '#/components/schemas/ListenV1Dictation'
            encoding:
              $ref: '#/components/schemas/ListenV1Encoding'
            endpointing:
              $ref: '#/components/schemas/ListenV1Endpointing'
            extra:
              $ref: '#/components/schemas/ListenV1Extra'
            interim_results:
              $ref: '#/components/schemas/ListenV1InterimResults'
            keyterm:
              $ref: '#/components/schemas/ListenV1Keyterm'
            keywords:
              $ref: '#/components/schemas/ListenV1Keywords'
            language:
              $ref: '#/components/schemas/ListenV1Language'
            mip_opt_out:
              $ref: '#/components/schemas/ListenV1MipOptOut'
            model:
              $ref: '#/components/schemas/ListenV1Model'
            multichannel:
              $ref: '#/components/schemas/ListenV1Multichannel'
            numerals:
              $ref: '#/components/schemas/ListenV1Numerals'
            profanity_filter:
              $ref: '#/components/schemas/ListenV1ProfanityFilter'
            punctuate:
              $ref: '#/components/schemas/ListenV1Punctuate'
            redact:
              $ref: '#/components/schemas/ListenV1Redact'
            replace:
              $ref: '#/components/schemas/ListenV1Replace'
            sample_rate:
              $ref: '#/components/schemas/ListenV1SampleRate'
            search:
              $ref: '#/components/schemas/ListenV1Search'
            smart_format:
              $ref: '#/components/schemas/ListenV1SmartFormat'
            tag:
              $ref: '#/components/schemas/ListenV1Tag'
            utterance_end_ms:
              $ref: '#/components/schemas/ListenV1UtteranceEndMs'
            vad_events:
              $ref: '#/components/schemas/ListenV1VadEvents'
            version:
              $ref: '#/components/schemas/ListenV1Version'
        headers:
          type: object
          properties:
            Authorization:
              type: string
    publish:
      operationId: listen-v-1-publish
      summary: Server messages
      message:
        oneOf:
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-server-0-ListenV1Results
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-server-1-ListenV1Metadata
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-server-2-ListenV1UtteranceEnd
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-server-3-ListenV1SpeechStarted
    subscribe:
      operationId: listen-v-1-subscribe
      summary: Client messages
      message:
        oneOf:
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-client-0-ListenV1Media
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-client-1-ListenV1Finalize
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-client-2-ListenV1CloseStream
          - $ref: >-
              #/components/messages/subpackage_listen/v1.listen.v1-client-3-ListenV1KeepAlive
servers:
  Production:
    url: wss://api.deepgram.com/
    protocol: wss
    x-default: true
  Agent:
    url: wss://api.deepgram.com/
    protocol: wss
components:
  messages:
    subpackage_listen/v1.listen.v1-server-0-ListenV1Results:
      name: ListenV1Results
      title: ListenV1Results
      description: Receive transcription results
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1Results'
    subpackage_listen/v1.listen.v1-server-1-ListenV1Metadata:
      name: ListenV1Metadata
      title: ListenV1Metadata
      description: Receive metadata about the transcription
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1Metadata'
    subpackage_listen/v1.listen.v1-server-2-ListenV1UtteranceEnd:
      name: ListenV1UtteranceEnd
      title: ListenV1UtteranceEnd
      description: Receive an utterance end event
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1UtteranceEnd'
    subpackage_listen/v1.listen.v1-server-3-ListenV1SpeechStarted:
      name: ListenV1SpeechStarted
      title: ListenV1SpeechStarted
      description: Receive a speech started event
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1SpeechStarted'
    subpackage_listen/v1.listen.v1-client-0-ListenV1Media:
      name: ListenV1Media
      title: ListenV1Media
      description: Send audio or video data to be transcribed
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1Media'
    subpackage_listen/v1.listen.v1-client-1-ListenV1Finalize:
      name: ListenV1Finalize
      title: ListenV1Finalize
      description: Send a Finalize message to flush the WebSocket stream
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1Finalize'
    subpackage_listen/v1.listen.v1-client-2-ListenV1CloseStream:
      name: ListenV1CloseStream
      title: ListenV1CloseStream
      description: Send a CloseStream message to close the WebSocket stream
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1CloseStream'
    subpackage_listen/v1.listen.v1-client-3-ListenV1KeepAlive:
      name: ListenV1KeepAlive
      title: ListenV1KeepAlive
      description: Send a KeepAlive message to keep the WebSocket stream alive
      payload:
        $ref: '#/components/schemas/ListenV1_ListenV1KeepAlive'
  schemas:
    ListenV1Callback:
      description: Any type
      title: ListenV1Callback
    ListenV1CallbackMethod:
      type: string
      enum:
        - POST
        - GET
        - PUT
        - DELETE
      default: POST
      description: HTTP method by which the callback request will be made
      title: ListenV1CallbackMethod
    ListenV1Channels:
      description: Any type
      title: ListenV1Channels
    ListenV1DetectEntities:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Identifies and extracts key entities from content in submitted audio.
        Entities appear in final results. When enabled, Punctuation will also be
        enabled by default
      title: ListenV1DetectEntities
    ListenV1Diarize:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Defaults to `false`. Recognize speaker changes. Each word in the
        transcript will be assigned a speaker number starting at 0
      title: ListenV1Diarize
    ListenV1Dictation:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: Identify and extract key entities from content in submitted audio
      title: ListenV1Dictation
    ListenV1Encoding:
      type: string
      enum:
        - linear16
        - linear32
        - flac
        - alaw
        - mulaw
        - amr-nb
        - amr-wb
        - opus
        - ogg-opus
        - speex
        - g729
      description: Specify the expected encoding of your submitted audio
      title: ListenV1Encoding
    ListenV1Endpointing:
      description: Any type
      title: ListenV1Endpointing
    ListenV1Extra:
      description: Any type
      title: ListenV1Extra
    ListenV1InterimResults:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Specifies whether the streaming endpoint should provide ongoing
        transcription updates as more audio is received. When set to true, the
        endpoint sends continuous updates, meaning transcription results may
        evolve over time
      title: ListenV1InterimResults
    ListenV1Keyterm:
      description: Any type
      title: ListenV1Keyterm
    ListenV1Keywords:
      description: Any type
      title: ListenV1Keywords
    ListenV1Language:
      description: Any type
      title: ListenV1Language
    ListenV1MipOptOut:
      description: Any type
      title: ListenV1MipOptOut
    ListenV1Model:
      type: string
      enum:
        - nova-3
        - nova-3-general
        - nova-3-medical
        - nova-2
        - nova-2-general
        - nova-2-meeting
        - nova-2-finance
        - nova-2-conversationalai
        - nova-2-voicemail
        - nova-2-video
        - nova-2-medical
        - nova-2-drivethru
        - nova-2-automotive
        - nova
        - nova-general
        - nova-phonecall
        - nova-medical
        - enhanced
        - enhanced-general
        - enhanced-meeting
        - enhanced-phonecall
        - enhanced-finance
        - base
        - meeting
        - phonecall
        - finance
        - conversationalai
        - voicemail
        - video
        - custom
      description: AI model to use for the transcription
      title: ListenV1Model
    ListenV1Multichannel:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: Transcribe each audio channel independently
      title: ListenV1Multichannel
    ListenV1Numerals:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: Convert numbers from written format to numerical format
      title: ListenV1Numerals
    ListenV1ProfanityFilter:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Profanity Filter looks for recognized profanity and converts it to the
        nearest recognized non-profane word or removes it from the transcript
        completely
      title: ListenV1ProfanityFilter
    ListenV1Punctuate:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: Add punctuation and capitalization to the transcript
      title: ListenV1Punctuate
    ListenV1Redact:
      type: string
      enum:
        - 'true'
        - 'false'
        - pci
        - numbers
        - aggressive_numbers
        - ssn
      default: 'false'
      description: Redaction removes sensitive information from your transcripts
      title: ListenV1Redact
    ListenV1Replace:
      description: Any type
      title: ListenV1Replace
    ListenV1SampleRate:
      description: Any type
      title: ListenV1SampleRate
    ListenV1Search:
      description: Any type
      title: ListenV1Search
    ListenV1SmartFormat:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Apply formatting to transcript output. When set to true, additional
        formatting will be applied to transcripts to improve readability
      title: ListenV1SmartFormat
    ListenV1Tag:
      description: Any type
      title: ListenV1Tag
    ListenV1UtteranceEndMs:
      description: Any type
      title: ListenV1UtteranceEndMs
    ListenV1VadEvents:
      type: string
      enum:
        - 'true'
        - 'false'
      default: 'false'
      description: >-
        Indicates that speech has started. You'll begin receiving Speech Started
        messages upon speech starting
      title: ListenV1VadEvents
    ListenV1Version:
      description: Any type
      title: ListenV1Version
    ChannelsListenV1MessagesListenV1ResultsType:
      type: string
      enum:
        - Results
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1ResultsType
    ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItemsWordsItems:
      type: object
      properties:
        word:
          type: string
          description: The word of the transcription
        start:
          type: number
          format: double
          description: The start time of the word
        end:
          type: number
          format: double
          description: The end time of the word
        confidence:
          type: number
          format: double
          description: The confidence of the word
        language:
          type: string
          description: The language of the word
        punctuated_word:
          type: string
          description: The punctuated word of the word
        speaker:
          type: integer
          description: The speaker of the word
      required:
        - word
        - start
        - end
        - confidence
      title: >-
        ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItemsWordsItems
    ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItems:
      type: object
      properties:
        transcript:
          type: string
          description: The transcript of the transcription
        confidence:
          type: number
          format: double
          description: The confidence of the transcription
        languages:
          type: array
          items:
            type: string
        words:
          type: array
          items:
            $ref: >-
              #/components/schemas/ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItemsWordsItems
      required:
        - transcript
        - confidence
        - words
      title: ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItems
    ChannelsListenV1MessagesListenV1ResultsChannel:
      type: object
      properties:
        alternatives:
          type: array
          items:
            $ref: >-
              #/components/schemas/ChannelsListenV1MessagesListenV1ResultsChannelAlternativesItems
      required:
        - alternatives
      title: ChannelsListenV1MessagesListenV1ResultsChannel
    ChannelsListenV1MessagesListenV1ResultsMetadataModelInfo:
      type: object
      properties:
        name:
          type: string
          description: The name of the model
        version:
          type: string
          description: The version of the model
        arch:
          type: string
          description: The arch of the model
      required:
        - name
        - version
        - arch
      title: ChannelsListenV1MessagesListenV1ResultsMetadataModelInfo
    ChannelsListenV1MessagesListenV1ResultsMetadata:
      type: object
      properties:
        request_id:
          type: string
          description: The request ID
        model_info:
          $ref: >-
            #/components/schemas/ChannelsListenV1MessagesListenV1ResultsMetadataModelInfo
        model_uuid:
          type: string
          description: The model UUID
      required:
        - request_id
        - model_info
        - model_uuid
      title: ChannelsListenV1MessagesListenV1ResultsMetadata
    ChannelsListenV1MessagesListenV1ResultsEntitiesItems:
      type: object
      properties:
        label:
          type: string
          description: >-
            The type/category of the entity (e.g., NAME, PHONE_NUMBER,
            EMAIL_ADDRESS, ORGANIZATION, CARDINAL)
        value:
          type: string
          description: The formatted text representation of the entity
        raw_value:
          type: string
          description: >-
            The original spoken text of the entity (present when formatting is
            enabled)
        confidence:
          type: number
          format: double
          description: The confidence score of the entity detection
        start_word:
          type: integer
          description: >-
            The index of the first word of the entity in the transcript
            (inclusive)
        end_word:
          type: integer
          description: >-
            The index of the last word of the entity in the transcript
            (exclusive)
      required:
        - label
        - value
        - raw_value
        - confidence
        - start_word
        - end_word
      title: ChannelsListenV1MessagesListenV1ResultsEntitiesItems
    ListenV1_ListenV1Results:
      type: object
      properties:
        type:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1ResultsType'
          description: Message type identifier
        channel_index:
          type: array
          items:
            type: integer
          description: The index of the channel
        duration:
          type: number
          format: double
          description: The duration of the transcription
        start:
          type: number
          format: double
          description: The start time of the transcription
        is_final:
          type: boolean
          description: Whether the transcription is final
        speech_final:
          type: boolean
          description: Whether the transcription is speech final
        channel:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1ResultsChannel'
        metadata:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1ResultsMetadata'
        from_finalize:
          type: boolean
          description: Whether the transcription is from a finalize message
        entities:
          type: array
          items:
            $ref: >-
              #/components/schemas/ChannelsListenV1MessagesListenV1ResultsEntitiesItems
          description: >-
            Extracted entities from the audio when detect_entities is enabled.
            Only present in is_final messages. Returns an empty array if no
            entities are detected
      required:
        - type
        - channel_index
        - duration
        - start
        - channel
        - metadata
      title: ListenV1_ListenV1Results
    ChannelsListenV1MessagesListenV1MetadataType:
      type: string
      enum:
        - Metadata
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1MetadataType
    ListenV1_ListenV1Metadata:
      type: object
      properties:
        type:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1MetadataType'
          description: Message type identifier
        transaction_key:
          type: string
          description: The transaction key
        request_id:
          type: string
          format: uuid
          description: The request ID
        sha256:
          type: string
          description: The sha256
        created:
          type: string
          description: The created
        duration:
          type: number
          format: double
          description: The duration
        channels:
          type: integer
          description: The channels
      required:
        - type
        - transaction_key
        - request_id
        - sha256
        - created
        - duration
        - channels
      title: ListenV1_ListenV1Metadata
    ChannelsListenV1MessagesListenV1UtteranceEndType:
      type: string
      enum:
        - UtteranceEnd
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1UtteranceEndType
    ListenV1_ListenV1UtteranceEnd:
      type: object
      properties:
        type:
          $ref: >-
            #/components/schemas/ChannelsListenV1MessagesListenV1UtteranceEndType
          description: Message type identifier
        channel:
          type: array
          items:
            type: integer
          description: The channel
        last_word_end:
          type: number
          format: double
          description: The last word end
      required:
        - type
        - channel
        - last_word_end
      title: ListenV1_ListenV1UtteranceEnd
    ChannelsListenV1MessagesListenV1SpeechStartedType:
      type: string
      enum:
        - SpeechStarted
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1SpeechStartedType
    ListenV1_ListenV1SpeechStarted:
      type: object
      properties:
        type:
          $ref: >-
            #/components/schemas/ChannelsListenV1MessagesListenV1SpeechStartedType
          description: Message type identifier
        channel:
          type: array
          items:
            type: integer
          description: The channel
        timestamp:
          type: number
          format: double
          description: The timestamp
      required:
        - type
        - channel
        - timestamp
      title: ListenV1_ListenV1SpeechStarted
    ListenV1_ListenV1Media:
      type: string
      format: binary
      title: ListenV1_ListenV1Media
    ChannelsListenV1MessagesListenV1FinalizeType:
      type: string
      enum:
        - Finalize
        - CloseStream
        - KeepAlive
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1FinalizeType
    ListenV1_ListenV1Finalize:
      type: object
      properties:
        type:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1FinalizeType'
          description: Message type identifier
      required:
        - type
      title: ListenV1_ListenV1Finalize
    ChannelsListenV1MessagesListenV1CloseStreamType:
      type: string
      enum:
        - Finalize
        - CloseStream
        - KeepAlive
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1CloseStreamType
    ListenV1_ListenV1CloseStream:
      type: object
      properties:
        type:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1CloseStreamType'
          description: Message type identifier
      required:
        - type
      title: ListenV1_ListenV1CloseStream
    ChannelsListenV1MessagesListenV1KeepAliveType:
      type: string
      enum:
        - Finalize
        - CloseStream
        - KeepAlive
      description: Message type identifier
      title: ChannelsListenV1MessagesListenV1KeepAliveType
    ListenV1_ListenV1KeepAlive:
      type: object
      properties:
        type:
          $ref: '#/components/schemas/ChannelsListenV1MessagesListenV1KeepAliveType'
          description: Message type identifier
      required:
        - type
      title: ListenV1_ListenV1KeepAlive

```