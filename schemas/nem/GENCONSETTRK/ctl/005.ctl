-- ************************************************************
-- Title:	PUBLIC_DVD_GENCONSETTRK_201910.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS GENCONSETTRK owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_GENCONSETTRK_201910010000.CSV
APPEND INTO TABLE GENCONSETTRK
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
GENCONSETID CHAR(20),
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
DESCRIPTION CHAR(256),
AUTHORISEDBY CHAR(15),
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
COVERAGE CHAR(64),
MODIFICATIONS CHAR(256),
SYSTEMNORMAL CHAR(1),
OUTAGE CHAR(256))
-- End of Script
